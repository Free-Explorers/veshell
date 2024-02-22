import 'package:fast_immutable_collections/fast_immutable_collections.dart';
import 'package:freedesktop_desktop_entry/freedesktop_desktop_entry.dart';
import 'package:riverpod_annotation/riverpod_annotation.dart';
import 'package:shell/screen/provider/focused_screen.dart';
import 'package:shell/screen/provider/screen_state.dart';
import 'package:shell/shared/provider/persistent_json_by_folder.dart';
import 'package:shell/wayland/model/event/destroy_surface/destroy_surface.serializable.dart';
import 'package:shell/wayland/model/event/wayland_event.serializable.dart';
import 'package:shell/wayland/model/request/close_window/close_window.serializable.dart';
import 'package:shell/wayland/model/wl_surface.dart';
import 'package:shell/wayland/model/xdg_surface.dart';
import 'package:shell/wayland/provider/surface.manager.dart';
import 'package:shell/wayland/provider/wayland.manager.dart';
import 'package:shell/wayland/provider/xdg_toplevel_state.dart';
import 'package:shell/window/model/dialog_window.dart';
import 'package:shell/window/model/persistent_window.serializable.dart';
import 'package:shell/window/model/window_id.dart';
import 'package:shell/window/provider/dialog_list_for_window.dart';
import 'package:shell/window/provider/dialog_window_state.dart';
import 'package:shell/window/provider/persistant_window_state.dart';
import 'package:shell/window/provider/surface_window_map.dart';
import 'package:shell/workspace/provider/window_workspace_map.dart';
import 'package:shell/workspace/provider/workspace_state.dart';
import 'package:uuid/uuid.dart';

part 'window.manager.g.dart';

/// Window manager
@Riverpod(keepAlive: true)
class WindowManager extends _$WindowManager {
  final _uuidGenerator = const Uuid();

  ISet<PersistentWindowId> get _persistentWindowSet =>
      state.whereType<PersistentWindowId>().toISet();

  @override
  ISet<WindowId> build() {
    ref
      ..listen(
        newXdgToplevelSurfaceProvider,
        (_, next) async {
          if (next case AsyncData(value: final SurfaceId surfaceId)) {
            onNewToplevel(surfaceId);
          }
        },
      )
      ..listen(waylandManagerProvider, (_, next) {
        if (next case AsyncData(value: final DestroySurfaceEvent event)) {
          _onSurfaceIsDestroyed(event.message);
        }
      });

    final intialSet = ref
            .read(persistentJsonByFolderProvider)
            .requireValue['Window']
            ?.keys
            .map<WindowId>(
              PersistentWindowId.new,
            )
            .toISet() ??
        <WindowId>{}.lock;

    return intialSet;
  }

  /// Create persistent window for desktop entry
  PersistentWindowId createPersistentWindowForDesktopEntry(
    LocalizedDesktopEntry entry,
  ) {
    final windowId = PersistentWindowId(_uuidGenerator.v4());

    final persistentWindow = PersistentWindow(
      windowId: windowId,
      appId: entry.desktopEntry.id ?? '',
      title: entry.desktopEntry.id ?? '',
      isWaitingForSurface: true,
    );

    ref
        .read(PersistentWindowStateProvider(windowId).notifier)
        .initialize(persistentWindow);

    state = state.add(windowId);
    return windowId;
  }

  /// new toplevel handler
  ///
  /// This is called when a new toplevel surface is created
  /// it first search for a waiting persistent window
  void onNewToplevel(SurfaceId surfaceId) {
    final toplevelState = ref.read(xdgToplevelStateProvider(surfaceId));

    for (final windowId in _persistentWindowSet) {
      final window = ref.read(persistentWindowStateProvider(windowId));
      if (window.appId == toplevelState.appId && window.isWaitingForSurface) {
        ref.read(persistentWindowStateProvider(windowId).notifier).initialize(
              window.copyWith(
                title: toplevelState.title,
                surfaceId: surfaceId,
                isWaitingForSurface: false,
              ),
            );
        return;
      }
    }
    if (toplevelState.parentSurfaceId != null) {
      _createDialogWindowForSurface(toplevelState);
      return;
    }
    // create a new window
    _createPersistentWindowForSurface(toplevelState);
  }

  _createPersistentWindowForSurface(XdgToplevelSurface toplevelState) {
    // create a new window
    final windowId = PersistentWindowId(_uuidGenerator.v4());

    final persistentWindow = PersistentWindow(
      windowId: windowId,
      appId: toplevelState.appId,
      title: toplevelState.title,
      surfaceId: toplevelState.surfaceId,
    );

    ref
        .read(persistentWindowStateProvider(windowId).notifier)
        .initialize(persistentWindow);

    state = state.add(windowId);

    final currentScreenId = ref.read(focusedScreenProvider);
    final screenState = ref.read(screenStateProvider(currentScreenId));

    ref
        .read(
          workspaceStateProvider(
            screenState.workspaceList[screenState.selectedIndex],
          ).notifier,
        )
        .addWindow(windowId);
  }

  _createDialogWindowForSurface(XdgToplevelSurface toplevelState) {
    // create a new window
    final windowId = DialogWindowId(_uuidGenerator.v4());

    final dialogWindow = DialogWindow(
      windowId: windowId,
      appId: toplevelState.appId,
      title: toplevelState.title,
      surfaceId: toplevelState.surfaceId,
      parentSurfaceId: toplevelState.parentSurfaceId!,
    );

    ref
        .read(dialogWindowStateProvider(windowId).notifier)
        .initialize(dialogWindow);

    state = state.add(windowId);
    final parentWindowId =
        ref.read(surfaceWindowMapProvider).get(toplevelState.parentSurfaceId!)!;

    ref.read(dialogListForWindowProvider.notifier).add(
          parentWindowId,
          windowId,
        );
  }

  _onSurfaceIsDestroyed(DestroySurfaceMessage message) {
    print('onSurfaceIsDestroyed $message');
    if (ref.read(surfaceWindowMapProvider).get(message.surfaceId)
        case final WindowId windowId) {
      switch (windowId) {
        case PersistentWindowId():
          ref
              .read(persistentWindowStateProvider(windowId).notifier)
              .onSurfaceIsDestroyed();
        case DialogWindowId():
          ref
              .read(dialogWindowStateProvider(windowId).notifier)
              .onSurfaceIsDestroyed();

          final dialogWindow = ref.read(dialogWindowStateProvider(windowId));

          ref.read(dialogListForWindowProvider.notifier).remove(
                ref
                    .read(surfaceWindowMapProvider)
                    .get(dialogWindow.parentSurfaceId)!,
                dialogWindow.windowId,
              );
          _removeWindow(windowId);
        case EphemeralWindowId():
        // TODO: Handle this case.
      }
    }
  }

  void _removeWindow(WindowId windowId) {
    state = state.remove(windowId);
    switch (windowId) {
      case PersistentWindowId():
        ref.read(persistentWindowStateProvider(windowId).notifier).dispose();
      case DialogWindowId():
        ref.read(dialogWindowStateProvider(windowId).notifier).dispose();
      case EphemeralWindowId():
    }
  }

  void closeWindow(WindowId windowId) {
    switch (windowId) {
      case PersistentWindowId():
        final persistentWindow =
            ref.read(persistentWindowStateProvider(windowId));
        if (persistentWindow.surfaceId != null) {
          closeWindowSurface(persistentWindow.surfaceId!);
        } else {
          _removeWindow(windowId);
          final workspaceId =
              ref.read(windowWorkspaceMapProvider).get(windowId);
          if (workspaceId != null) {
            ref
                .read(workspaceStateProvider(workspaceId).notifier)
                .removeWindow(windowId);
          }
        }
      case DialogWindowId():
        closeWindowSurface(
          ref.read(dialogWindowStateProvider(windowId)).surfaceId,
        );
      case EphemeralWindowId():
    }
  }

  /// Close surface for window
  void closeWindowSurface(SurfaceId surfaceId) {
    ref.read(waylandManagerProvider.notifier).request(
          CloseWindowRequest(
            message: CloseWindowMessage(
              surfaceId: surfaceId,
            ),
          ),
        );
  }
}
