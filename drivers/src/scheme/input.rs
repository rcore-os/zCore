use super::Scheme;
use crate::DeviceResult;

pub trait InputScheme: Scheme {
    type InputState;

    fn state(&self) -> DeviceResult<Self::InputState>;
}
