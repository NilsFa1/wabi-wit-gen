#[path = "../../sdk/core-rust/generated.rs"]
pub mod generated;

use generated::{GreetError, Greeting};
use wabi_abi_rt::host::AbiStream;

pub struct Core;
impl generated::Implementation for Core {
    const CORE_INFO: &'static std::ffi::CStr = c"greeter-core/0.1.0";
    type Greeter = Greeter;
}

pub struct Greeter {
    prefix: String,
}

impl generated::GreeterResource for Greeter {
    fn new(prefix: String) -> Self {
        Greeter { prefix }
    }
    async fn greet(&self, name: String) -> Result<Greeting, GreetError> {
        if name.is_empty() {
            return Err(GreetError::EmptyName);
        }
        Ok(Greeting { text: format!("{} {name}", self.prefix), count: 1 })
    }
    fn events(&self) -> AbiStream<Greeting> {
        Box::pin(futures::stream::empty())
    }
}
