use smithay::reexports::calloop::channel;
use std::cell::RefCell;
use std::rc::Rc;

use crate::flutter_engine::platform_channels::binary_messenger::{
    BinaryMessageHandler, BinaryMessenger, BinaryReply,
};
use crate::flutter_engine::platform_channels::encodable_value::EncodableValue;
use crate::flutter_engine::platform_channels::engine_method_result::EngineMethodResult;
use crate::flutter_engine::platform_channels::method_call::MethodCall;
use crate::flutter_engine::platform_channels::method_codec::MethodCodec;
use crate::flutter_engine::platform_channels::method_result::MethodResult;

type MethodCallHandler<T> = Option<Box<dyn FnMut(MethodCall<T>, Box<dyn MethodResult<T>>)>>;

pub struct MethodChannel<T = EncodableValue> {
    messenger: Rc<RefCell<dyn BinaryMessenger>>,
    name: String,
    codec: Rc<dyn MethodCodec<T>>,
}

impl<T: 'static> MethodChannel<T> {
    pub fn new(
        messenger: Rc<RefCell<dyn BinaryMessenger>>,
        name: String,
        codec: Rc<dyn MethodCodec<T>>,
    ) -> Self {
        Self {
            messenger,
            name,
            codec,
        }
    }

    pub fn invoke_method(
        &mut self,
        method: &str,
        arguments: Option<Box<T>>,
        result: Option<Box<dyn MethodResult<T>>>,
    ) {
        let method_call = MethodCall::new(method.to_string(), arguments);
        let message = self.codec.encode_method_call(&method_call);
        let mut result = if let Some(result) = result {
            result
        } else {
            self.messenger.borrow_mut().send(&self.name, &message, None);
            return;
        };

        let codec = self.codec.clone();
        let channel_name = self.name.clone();

        let reply_handler: BinaryReply = Some(Box::new(move |response: Option<&[u8]>| {
            let response = if let Some(response) = response {
                response
            } else {
                result.not_implemented();
                return;
            };
            codec.decode_and_process_response_envelope(response, result.as_mut());
        }));

        self.messenger
            .borrow_mut()
            .send(&channel_name, &message, reply_handler);
    }

    pub fn set_method_call_handler(&mut self, handler: MethodCallHandler<T>) {
        let mut handler = if let Some(handler) = handler {
            handler
        } else {
            self.messenger
                .borrow_mut()
                .set_message_handler(&self.name, None);
            return;
        };

        let codec = self.codec.clone();
        let channel_name = self.name.clone();

        let binary_handler: BinaryMessageHandler =
            Some(Box::new(move |message: &[u8], reply: BinaryReply| {
                let mut result = EngineMethodResult::new(reply, codec.clone());
                let method_call = codec.decode_method_call(message);
                if let Some(method_call) = method_call {
                    handler(method_call, Box::new(result));
                } else {
                    eprintln!(
                        "Unable to construct method call from message on channel {channel_name}"
                    );
                    result.not_implemented();
                }
            }));
        self.messenger
            .borrow_mut()
            .set_message_handler(&self.name, binary_handler);
    }

    #[allow(clippy::type_complexity)]
    pub fn set_method_call_mpsc_channel(
        &mut self,
        channel: Option<channel::Sender<(MethodCall<T>, Box<dyn MethodResult<T>>)>>,
    ) {
        let handler: MethodCallHandler<T> = channel.map(|channel| {
            Box::new(move |method_call, result| {
                channel.send((method_call, result)).unwrap();
            }) as Box<dyn FnMut(MethodCall<T>, Box<dyn MethodResult<T>>)>
        });
        self.set_method_call_handler(handler);
    }
}
