#![warn(missing_debug_implementations)]

pub use cuda_sys::wrapper::handle::Device;

pub mod executor;
pub mod shape;
pub mod tensor;
pub mod tester;

mod planner;

//TODO make this private again?
pub mod kernels;
