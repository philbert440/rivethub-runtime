// compile-flags: --crate-name rh
#![allow(dead_code)]

mod tokio {
    pub mod runtime {
        pub struct Builder;

        impl Builder {
            pub fn new_multi_thread() -> Builder {
                Builder
            }

            pub fn new_current_thread() -> Builder {
                Builder
            }

            pub fn enable_all(self) -> Builder {
                self
            }

            pub fn build(self) {}
        }

        pub struct Runtime;

        impl Runtime {
            pub fn new() -> Runtime {
                Runtime
            }
        }
    }
}

pub fn the_rh_binary_may_build_a_runtime() {
    let _ = tokio::runtime::Builder::new_multi_thread();
    let _ = tokio::runtime::Builder::new_current_thread();
    let _ = tokio::runtime::Runtime::new();
}

pub fn main() {}
