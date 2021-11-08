use std::collections::{HashMap, HashSet};

use bytemuck::cast_slice;

use cuda_sys::bindings::{cudnnActivationMode_t, cudnnOpTensorOp_t};
use cuda_sys::wrapper::descriptor::{ActivationDescriptor, ConvolutionDescriptor, TensorOpDescriptor};
use cuda_sys::wrapper::group::{FusedConvolutionArgs, TensorOpArgs};
use cuda_sys::wrapper::handle::CudnnHandle;
use cuda_sys::wrapper::mem::device::DeviceMem;
use cuda_sys::wrapper::operation::STANDARD_CONV_ALGO;
use nn_graph::graph::{ConvDetails, Graph, Operation, Value};
use nn_graph::optimizer::find_single_use_values;
use nn_graph::shape::ConcreteShape;

use crate::executor::Step;
use crate::shape::StridedShape;
use crate::tensor::Tensor;

pub struct Planner<'a> {
    handle: &'a mut CudnnHandle,
    graph: &'a Graph,
    batch_size: usize,

    single_use: HashSet<Value>,

    map: HashMap<Value, Tensor>,
    plan: Vec<Step>,
}

impl<'a> Planner<'a> {
    pub fn new(handle: &'a mut CudnnHandle, graph: &'a Graph, batch_size: usize) -> Self {
        let single_use = find_single_use_values(graph);

        Planner {
            handle,
            graph,
            batch_size,
            single_use,
            map: Default::default(),
            plan: vec![],
        }
    }

    pub fn copy_output(&mut self, index: usize, value: Value) -> Tensor {
        let tensor = self.map.get(&value).unwrap();
        self.plan.push(Step::CopyOutput { index, tensor: tensor.view() });
        tensor.view()
    }

    pub fn finish(self) -> Vec<Step> {
        self.plan
    }

    pub fn visit(&mut self, value: Value) -> Tensor {
        if let Some(result) = self.map.get(&value) {
            return result.view();
        }

        if let Some(result) = self.visit_fused_conv(value) {
            let prev = self.map.insert(value, result.view());
            assert!(prev.is_none());
            return result;
        }

        let result_info = &self.graph[value];
        let result_shape = result_info.shape.eval(self.batch_size);

        let result: Tensor = match &result_info.operation {
            &Operation::Input { index } => {
                let buffer = self.alloc_buffer(result_shape.size());
                self.plan.push(Step::CopyInput { index, mem: buffer.view() });
                Tensor::new(buffer, StridedShape::new_simple(result_shape.dims))
            }
            Operation::Constant { data } => {
                let buffer = self.alloc_buffer(result_shape.size());

                // safety: we just allocated the buffer so we're the only one that can mutate it
                unsafe {
                    buffer.copy_from_host(cast_slice(&**data));
                }

                Tensor::new(buffer, StridedShape::new_simple(result_shape.dims))
            }
            &Operation::View { input } => {
                let input_tensor = self.visit(input);

                let new_shape = input_tensor.shape.view(result_shape.dims.clone())
                    .unwrap_or_else(|| panic!("Cannot view shape {:?} as {:?}", input_tensor.shape, result_shape));

                Tensor::new(input_tensor.mem.view(), new_shape)
            }
            &Operation::Slice { input, axis, start, end } => {
                // Steps to slice a tensor:
                //  * use the new shape
                //  * keep the old strides
                //  * offset initial pointer to account for `start`
                //  * limit the buffer length based on the new size

                let input_tensor = self.visit(input);
                let result_shape = input_tensor.shape.slice(axis, start, end);

                let start_bytes = result_shape.strides()[axis] * start * 4;
                let len_bytes = result_shape.strided_size() * 4;

                let mem = input_tensor.mem.slice_bytes(start_bytes, len_bytes);
                Tensor::new(mem, result_shape)
            }
            &Operation::Conv { .. } =>
                unreachable!("conv should have been handled earlier by the fuser"),
            &Operation::Add { left, right, subtract } => {
                self.visit_op(result_shape, left, right, cudnnOpTensorOp_t::CUDNN_OP_TENSOR_ADD, subtract)
            }
            &Operation::Mul { left, right } => {
                self.visit_op(result_shape, left, right, cudnnOpTensorOp_t::CUDNN_OP_TENSOR_MUL, false)
            }
            &Operation::Clamp { input, min, max } => {
                let mut curr = self.visit(input).view();
                if min != f32::NEG_INFINITY {
                    curr = self.visit_clamp(result_shape.clone(), &curr, min, cudnnOpTensorOp_t::CUDNN_OP_TENSOR_MAX);
                }
                if max != f32::INFINITY {
                    curr = self.visit_clamp(result_shape, &curr, max, cudnnOpTensorOp_t::CUDNN_OP_TENSOR_MIN);
                }
                curr
            }
        };

        let prev = self.map.insert(value, result.view());
        assert!(prev.is_none());

        result
    }

    fn visit_fused_conv(&mut self, value: Value) -> Option<Tensor> {
        let mut curr = value;
        let graph = self.graph;

        // clamp(curr, 0, inf)?
        let act_mode = if let &Operation::Clamp { input, min, max } = &graph[curr].operation {
            if !self.single_use.contains(&input) || min != 0.0 && max != f32::INFINITY {
                return None;
            }
            cudnnActivationMode_t::CUDNN_ACTIVATION_RELU
        } else {
            cudnnActivationMode_t::CUDNN_ACTIVATION_IDENTITY
        };

        let mut bias = None;
        let mut res = None;

        loop {
            if let &Operation::Add { left, right, subtract: false } = &graph[curr].operation {
                if !self.single_use.contains(&left) {
                    return None;
                }

                //TODO check in advance whether outputs will be densely strided, instead of asserting it at the end
                //TODO try visiting both left and right for the continuation

                if graph[left].shape == graph[right].shape {
                    // has to be res
                    if res.is_none() {
                        res = Some(right);
                    } else {
                        return None;
                    }
                } else if graph[left].shape.all_ones_except(1) == graph[right].shape {
                    // has to be bias
                    if bias.is_none() {
                        bias = Some(right);
                    } else {
                        return None;
                    }
                } else {
                    return None;
                }

                curr = left;
            } else {
                break;
            }
        }

        if let &Operation::Conv { input, filter, details } = &graph[curr].operation {
            if let Some(res) = res {
                if graph[input].shape.eval(self.batch_size) != graph[res].shape.eval(self.batch_size) {
                    return None;
                }
            }

            let input = self.visit(input);
            let filter = self.visit(filter);
            let bias = bias.map(|bias| self.visit(bias));
            let res = res.map(|res| self.visit(res));

            if let Some(res) = &res {
                //TODO this should be checked before we actually start fusing
                assert_eq!(res.shape, input.shape, "Input and res shapes and strides (!) must match.", );
            }

            let bias = bias.unwrap_or_else(|| {
                let bias_shape = StridedShape::new(vec![1, details.output_channels, 1, 1], vec![1, 1, 1, 1]);
                Tensor::new(self.alloc_buffer(details.output_channels), bias_shape)
            });

            let output_shape = graph[curr].shape.eval(self.batch_size);
            let output = Tensor::new(
                self.alloc_buffer(output_shape.size()),
                StridedShape::new_simple(output_shape.dims),
            );

            let input_desc = input.descriptor();
            let output_desc = output.descriptor();
            let filter_desc = filter.filter_descriptor();

            let padding = details.padding as i32;
            let conv_desc = ConvolutionDescriptor::new(padding, padding, 1, 1, 1, 1);

            let algo = STANDARD_CONV_ALGO;
            let work_size_bytes = conv_desc.workspace_size(self.handle, algo, &input_desc, &filter_desc, &output_desc);
            let work_mem = DeviceMem::alloc(work_size_bytes, self.handle.device());

            let act_desc = ActivationDescriptor::new(act_mode, 0.0);

            let args = FusedConvolutionArgs {
                conv_desc,
                algo,
                work_mem,
                filter_desc,
                filter_mem: filter.mem,
                input_desc,
                input_mem: input.mem,
                res_mem: res.map(|res| res.mem),
                bias_desc: bias.descriptor(),
                bias_mem: bias.mem,
                act_desc,
                output_desc,
                output_mem: output.mem.view(),
            };

            self.plan.push(Step::Conv { details, args });
            Some(output)
        } else {
            None
        }
    }

    fn visit_op(&mut self, result_shape: ConcreteShape, left: Value, right: Value, op: cudnnOpTensorOp_t, negate_right: bool) -> Tensor {
        let op_desc = TensorOpDescriptor::new(op);
        let alpha_2 = if negate_right { -1.0 } else { 1.0 };

        let output = Tensor::new(
            self.alloc_buffer(result_shape.size()),
            StridedShape::new_simple(result_shape.dims),
        );

        let left = self.visit(left);
        let right = self.visit(right);

        let args = TensorOpArgs {
            op_desc,
            alpha_1: 1.0,
            input_1_desc: left.descriptor(),
            input_1_mem: left.mem.view(),
            alpha_2,
            input_2_desc: right.descriptor(),
            input_2_mem: right.mem.view(),
            beta: 0.0,
            output_desc: output.descriptor(),
            output_mem: output.mem.view(),
        };

        self.plan.push(Step::TensorOp { args });
        output
    }

    fn visit_clamp(&mut self, result_shape: ConcreteShape, input: &Tensor, limit: f32, op: cudnnOpTensorOp_t) -> Tensor {
        assert!(op == cudnnOpTensorOp_t::CUDNN_OP_TENSOR_MIN || op == cudnnOpTensorOp_t::CUDNN_OP_TENSOR_MAX);
        let op_desc = TensorOpDescriptor::new(op);

        // create a one-element tensor with the same rank as `left` to hold the limit value
        let right = Tensor::new(
            self.alloc_buffer(1),
            StridedShape::new_simple(vec![1; result_shape.rank()]),
        );

        // safety: we just allocated this memory so no one else can modify it
        unsafe {
            right.mem.copy_from_host(cast_slice(&[limit]));
        }

        let output = Tensor::new(
            self.alloc_buffer(result_shape.size()),
            StridedShape::new_simple(result_shape.dims).clone(),
        );

        let args = TensorOpArgs {
            op_desc,
            alpha_1: 1.0,
            input_1_desc: input.descriptor(),
            input_1_mem: input.mem.view(),
            alpha_2: 1.0,
            input_2_desc: right.descriptor(),
            input_2_mem: right.mem.view(),
            beta: 0.0,
            output_desc: output.descriptor(),
            output_mem: output.mem.view(),
        };

        self.plan.push(Step::TensorOp { args });
        output
    }

    fn alloc_buffer(&mut self, size: usize) -> DeviceMem {
        DeviceMem::alloc(size * 4, self.handle.device())
    }
}
