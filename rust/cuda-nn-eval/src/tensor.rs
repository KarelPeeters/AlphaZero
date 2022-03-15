use bytemuck::{cast_slice, cast_slice_mut};

use cuda_sys::bindings::{cublasOperation_t, cudnnOpTensorOp_t};
use cuda_sys::wrapper::descriptor::{TensorDescriptor, TensorOpDescriptor};
use cuda_sys::wrapper::group::MatMulArg;
use cuda_sys::wrapper::handle::{CudnnHandle, Device};
use cuda_sys::wrapper::mem::device::DevicePtr;
use cuda_sys::wrapper::operation::run_tensor_op;

use crate::shape::StridedShape;

/// A tensor allocated on the device.
///
/// Cloning this type does not copy the underlying memory.
#[derive(Debug, Clone)]
pub struct DeviceTensor {
    pub ptr: DevicePtr,
    pub shape: StridedShape,
}

impl DeviceTensor {
    pub fn new(ptr: DevicePtr, shape: StridedShape) -> Self {
        DeviceTensor { ptr, shape }
    }

    pub fn alloc_simple(device: Device, shape: Vec<usize>) -> Self {
        let size = shape.iter().product::<usize>();
        let ptr = DevicePtr::alloc(device, size * 4);
        DeviceTensor::new(ptr, StridedShape::new_simple(shape))
    }

    pub fn device(&self) -> Device {
        self.ptr.device()
    }

    pub fn permute(&self, permutation: &[usize]) -> DeviceTensor {
        DeviceTensor::new(self.ptr.clone(), self.shape.permute(permutation))
    }

    pub fn slice(&self, axis: usize, start: usize, end: usize) -> DeviceTensor {
        // Steps to slice a tensor:
        //  * use the new shape
        //  * keep the old strides
        //  * offset initial pointer to account for `start`
        //  * limit the buffer length based on the new size
        let result_shape = self.shape.slice(axis, start, end);

        let start_bytes = result_shape.strides()[axis] * start * 4;
        let mem = self.ptr.offset(start_bytes as isize);

        DeviceTensor::new(mem, result_shape)
    }

    pub fn to_mat_mul_arg(&self) -> MatMulArg {
        assert_eq!(self.shape.rank(), 3);

        let inner_shape = StridedShape::new(self.shape.shape()[1..].to_vec(), self.shape.strides()[1..].to_vec());

        // whether the strides are col-major (true) or row-major (false)
        let col_major = if inner_shape.has_simple_strides() {
            false
        } else if inner_shape.permute(&[1, 0]).has_simple_strides() {
            true
        } else {
            panic!(
                "For now GPU matmul operand must be either col- or row-major, got {:?}",
                self
            )
        };

        let lead_axis = if col_major { 1 } else { 2 };

        MatMulArg {
            ptr: self.ptr.clone(),
            trans: if col_major {
                cublasOperation_t::CUBLAS_OP_N
            } else {
                cublasOperation_t::CUBLAS_OP_T
            },
            ld: self.shape.shape()[lead_axis] as i32,
            stride: self.shape.strides()[0] as i64,
        }
    }

    pub unsafe fn copy_simple_from_host(&self, buffer: &[f32]) {
        assert!(
            self.shape.has_simple_strides(),
            "Tensor must have simple strides for now, got {:?}",
            self.shape
        );
        self.ptr.copy_linear_from_host(cast_slice(buffer));
    }

    pub unsafe fn copy_simple_to_host(&self, buffer: &mut [f32]) {
        assert!(
            self.shape.has_simple_strides(),
            "Tensor must have simple strides, got {:?}",
            self.shape
        );
        self.ptr.copy_linear_to_host(cast_slice_mut(buffer));
    }

    pub unsafe fn copy_from(&self, other: &DeviceTensor) {
        assert_eq!(
            self.shape.shape(),
            other.shape.shape(),
            "Tensors must have the same shape: {:?} vs {:?}",
            self,
            other
        );

        if self.shape == other.shape && self.shape.has_dense_strides() {
            // if strides are dense and match we can just do a simple memcpy
            self.ptr.copy_linear_from_device(&other.ptr, self.shape.size())
        } else {
            // otherwise use the TensorOp restride trick
            restride_with_tensor_op(other, self);
        }
    }

    /// A (potentially) slower version of [Self::copy_from_host] that works for any strides,
    /// by potentially copying to an intermediate stage on the device.
    pub unsafe fn copy_from_host_staged(&self, buffer: &[f32]) {
        if self.shape.has_simple_strides() {
            self.copy_simple_from_host(buffer);
        } else {
            let stage = DeviceTensor::alloc_simple(self.device(), self.shape.shape().to_vec());
            stage.copy_simple_from_host(buffer);
            self.copy_from(&stage);
        }
    }

    /// A (potentially) slower version of [Self::copy_to_host] that works for any strides,
    /// by potentially copying to an intermediate stage on the device.
    pub unsafe fn copy_to_host_staged(&self, buffer: &mut [f32]) {
        if self.shape.has_simple_strides() {
            self.copy_simple_to_host(buffer);
        } else {
            let stage = DeviceTensor::alloc_simple(self.device(), self.shape.shape().to_vec());
            stage.copy_from(self);
            stage.copy_simple_to_host(buffer);
        }
    }
}

//TODO extract this function to somewhere more general, maybe even with fixed pre-allocation of the descriptors
unsafe fn restride_with_tensor_op(input: &DeviceTensor, output: &DeviceTensor) {
    let handle = CudnnHandle::new(input.device());

    let op_desc = TensorOpDescriptor::new(cudnnOpTensorOp_t::CUDNN_OP_TENSOR_ADD);

    // we don't need to initialize anything, since alpha_2 is already 0
    let zero = handle.device().alloc(4);
    let zero_desc = TensorDescriptor::new(vec![1, 1, 1, 1], vec![1, 1, 1, 1]);

    run_tensor_op(
        &handle,
        &op_desc,
        1.0,
        &input.shape.descriptor(),
        &input.ptr,
        0.0,
        &zero_desc,
        &zero,
        0.0,
        &output.shape.descriptor(),
        &output.ptr,
    );

    handle.stream().synchronize();
}
