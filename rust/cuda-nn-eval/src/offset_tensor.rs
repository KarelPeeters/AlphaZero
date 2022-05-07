use crate::shape::StridedShape;
use cuda_sys::bindings::cublasOperation_t;
use cuda_sys::wrapper::group::MatMulOperand;
use nn_graph::graph::SliceRange;
use std::fmt::Debug;

pub trait OffsetPtr: Debug + Clone {
    fn offset_bytes(self, offset: isize) -> Self;
}

/// A generic Tensor representation.
#[derive(Debug, Clone)]
pub struct PtrTensor<P> {
    shape: StridedShape,
    ptr: P,
}

impl<P> PtrTensor<P> {
    pub fn from_parts(ptr: P, shape: StridedShape) -> Self {
        PtrTensor { ptr, shape }
    }

    pub fn into_ptr(self) -> P {
        self.ptr
    }

    pub fn ptr(&self) -> &P {
        &self.ptr
    }

    pub fn shape(&self) -> &StridedShape {
        &self.shape
    }

    pub fn map_ptr<K>(self, f: impl FnOnce(P) -> K) -> PtrTensor<K> {
        PtrTensor::from_parts(f(self.ptr), self.shape)
    }
}

impl<P: OffsetPtr> PtrTensor<P> {
    fn offset(&self, offset: isize, shape: StridedShape) -> Self {
        Self::from_parts(self.ptr.clone().offset_bytes(4 * offset), shape)
    }

    pub fn permute(&self, permutation: &[usize]) -> Self {
        self.offset(0, self.shape.permute(permutation))
    }

    pub fn view(&self, new_shape: Vec<usize>) -> Self {
        self.offset(0, self.shape.view(new_shape).unwrap())
    }

    pub fn broadcast(&self, new_shape: Vec<usize>) -> Self {
        self.offset(0, self.shape.broadcast(new_shape))
    }

    pub fn slice(&self, axis: usize, range: SliceRange) -> Self {
        // use the new shape & strides (which only change along `axis`)
        let result_shape = self.shape.slice(axis, range);

        let offset = if result_shape.size() != 0 {
            // offset initial pointer to account for `start`
            result_shape.strides()[axis] * range.start as isize
        } else {
            0
        };

        self.offset(offset, result_shape)
    }

    pub fn index(&self, axis: usize, index: usize) -> Self {
        let mut new_shape = self.shape.shape().to_vec();
        new_shape.remove(axis);

        self.slice(axis, SliceRange::simple(index, index + 1)).view(new_shape)
    }

    pub fn flip(&self, axis: usize) -> Self {
        // invert the axis stride
        let result_shape = self.shape.flip(axis);

        let axis_len = self.shape.shape()[axis];
        let offset = if self.shape.size() != 0 && axis_len != 0 {
            // offset so index 0 gets the last element along the axis
            (axis_len - 1) as isize * self.shape.strides()[axis]
        } else {
            0
        };

        self.offset(offset, result_shape)
    }

    pub fn repeat_unary(&self, axis: usize, count: usize) -> Self {
        let result_shape = self.shape.repeat_unary(axis, count);
        self.offset(0, result_shape)
    }
}

impl<P: Clone> PtrTensor<P> {
    //TODO move this somewhere else, this is pretty random
    pub fn to_mat_mul_arg(&self) -> MatMulOperand<P> {
        assert_eq!(self.shape().rank(), 3);

        let inner_shape = StridedShape::new(self.shape().shape()[1..].to_vec(), self.shape().strides()[1..].to_vec());

        // whether the strides are col-major (true) or row-major (false)
        let col_major = if inner_shape.has_simple_strides() {
            false
        } else if inner_shape.permute(&[1, 0]).has_simple_strides() {
            true
        } else {
            panic!(
                "GPU matmul operand must be either col- or row-major, got {:?}",
                self.shape
            )
        };

        let lead_axis = if col_major { 1 } else { 2 };

        MatMulOperand {
            ptr: self.ptr().clone(),
            trans: if col_major {
                cublasOperation_t::CUBLAS_OP_N
            } else {
                cublasOperation_t::CUBLAS_OP_T
            },
            ld: self.shape().shape()[lead_axis] as i32,
            stride: self.shape().strides()[0] as i64,
        }
    }
}
