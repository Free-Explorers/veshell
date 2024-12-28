//! https://api.flutter.dev/flutter/services/SystemChannels/textInput-constant.html

use smithay::input::pointer::CursorIcon;
use smithay::reexports::calloop::channel::Event;
use tracing::info;

use crate::backend::Backend;
use crate::flutter_engine::platform_channels::method_call::MethodCall;
use crate::flutter_engine::platform_channels::method_result::MethodResult;
use crate::state::State;

use super::platform_channels::encodable_value::EncodableValue;

pub fn mouse_cursor_channel_method_call_handler<BackendData: Backend + 'static>(
    event: Event<(
        MethodCall<EncodableValue>,
        Box<dyn MethodResult<EncodableValue>>,
    )>,
    _: &mut (),
    data: &mut State<BackendData>,
) {
    if let Event::Msg((method_call, mut result)) = event {
        let arguments = method_call.arguments();
        if method_call.method() == "activateSystemCursor" {
            let args = arguments.unwrap().map().unwrap();
            let mut kind = None;
            let mut device: Option<i32> = None;
            for (key, value) in args.iter() {
                info!("{}", key.string().unwrap());
                let key_string = key.string().unwrap();
                if key_string == "kind" {
                    kind = value.string();
                }
                if key_string == "device" {
                    device = value.int32();
                }
            }
            if let (Some(kind), Some(_device)) = (kind, device) {
                let mut cursor_state = data.cursor_state.lock().unwrap();
                match kind {
                    "basic" => cursor_state.set_shape(CursorIcon::Default),
                    "click" => cursor_state.set_shape(CursorIcon::Pointer),
                    "text" => cursor_state.set_shape(CursorIcon::Text),
                    "verticalText" => cursor_state.set_shape(CursorIcon::VerticalText),
                    "wait" => cursor_state.set_shape(CursorIcon::Wait),
                    "progress" => cursor_state.set_shape(CursorIcon::Progress),
                    "alias" => cursor_state.set_shape(CursorIcon::Alias),
                    "copy" => cursor_state.set_shape(CursorIcon::Copy),
                    _ => cursor_state.set_shape(CursorIcon::Default),
                }

                return result.success(None);
            }
            return result.error(
                "no_device_or_kind".to_string(),
                "No device or kind provided".to_string(),
                None,
            );
        }
        result.not_implemented()
    }
}
