use crate::backend::Backend;
use crate::state::State;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{LoopHandle, RegistrationToken};
use std::time::Duration;

use super::VeshellKeyEvent;

type Callback<BackendData> = fn(VeshellKeyEvent, &mut State<BackendData>);

pub struct KeyRepeater<BackendData: Backend + 'static> {
    loop_handle: LoopHandle<'static, State<BackendData>>,
    timer_token: Option<RegistrationToken>,
    callback: Callback<BackendData>,
    repeating_event: Option<VeshellKeyEvent>,
}

impl<BackendData: Backend + 'static> KeyRepeater<BackendData> {
    pub fn new(
        loop_handle: LoopHandle<'static, State<BackendData>>,
        callback: Callback<BackendData>,
    ) -> Self {
        Self {
            loop_handle,
            timer_token: None,
            callback,
            repeating_event: None,
        }
    }

    pub fn down(&mut self, event: VeshellKeyEvent, repeat_delay: Duration, repeat_rate: Duration) {
        // Cancel any existing key repeat, we don't want to repeat multiple keys at once.
        self.cancel();
        self.repeating_event = Some(event);

        let timer = Timer::from_duration(repeat_delay);

        let callback = self.callback;
        let token = self
            .loop_handle
            .insert_source(timer, move |_, _, data| {
                callback(event, data);
                // Reschedule the timer over and over.
                TimeoutAction::ToDuration(repeat_rate)
            })
            .unwrap();

        self.timer_token = Some(token);
    }

    pub fn up(&mut self, event: VeshellKeyEvent) {
        let repeating_event = match self.repeating_event {
            Some(repeating_event) => repeating_event,
            None => return,
        };
        // Only stop the key repeat if the user releases the same key that caused the repeat.
        if repeating_event.key_code == event.key_code {
            self.cancel();
        }
    }

    fn cancel(&mut self) {
        if let Some(token) = self.timer_token.take() {
            self.loop_handle.remove(token);
        }
    }
}
