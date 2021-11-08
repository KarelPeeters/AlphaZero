use std::convert::TryInto;
use std::fmt::{Debug, Display, Formatter};
use std::ops::{Deref, Index};

use itertools::{Itertools, zip_eq};

use crate::shape::{Shape, Size};

#[derive(Clone)]
pub struct Graph {
    values: Vec<ValueInfo>,
    inputs: Vec<Value>,
    outputs: Vec<Value>,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct Value(usize);

#[derive(Debug, Clone)]
pub struct ValueInfo {
    pub shape: Shape,
    pub operation: Operation,
}

/// Wrapper type that prevents the Debug output from getting too large.
#[derive(Clone)]
pub struct ConstantData(Vec<f32>);

/// The core set of operations.
/// Many other operations can be implemented as combinations of these operators, a few examples:
/// * `ReLU` -> `Clip`
/// * `BatchNorm` -> `Bias,Conv,Bias,Conv` (can be fused into adjacent conv too)
/// * `Gemm` -> `Conv,Bias`
#[derive(Debug, Clone)]
pub enum Operation {
    /// A runtime-variable input.
    Input { index: usize },
    /// A constant build into the network.
    Constant { data: ConstantData },

    /// View a value as a different shape.
    View { input: Value },
    /// Slice the last three axis of a value, each with range `start[i]..end[i]`
    Slice { input: Value, axis: usize, start: usize, end: usize },

    /// The standard convolution operator.
    Conv { input: Value, filter: Value, details: ConvDetails },

    /// Elementwise add two values, with broadcasting on the right.
    Add { left: Value, right: Value, subtract: bool },
    /// Elementwise multiply two values, with broadcasting on the right value.
    Mul { left: Value, right: Value },

    /// Elementwise clip a value.
    Clamp { input: Value, min: f32, max: f32 },
}

impl Operation {
    pub fn inputs(&self) -> Vec<Value> {
        match self {
            Operation::Input { .. } => vec![],
            Operation::Constant { .. } => vec![],
            &Operation::View { input } => vec![input],
            &Operation::Slice { input, .. } => vec![input],
            &Operation::Conv { input, filter, .. } => vec![input, filter],
            &Operation::Add { left, right, .. } => vec![left, right],
            &Operation::Mul { left, right } => vec![left, right],
            &Operation::Clamp { input, .. } => vec![input],
        }
    }

    pub(crate) fn clone_map_inputs(&self, mut f: impl FnMut(Value) -> Value) -> Operation {
        match self {
            &Operation::Input { index } =>
                Operation::Input { index },
            Operation::Constant { data } =>
                Operation::Constant { data: data.clone() },
            &Operation::View { input } =>
                Operation::View { input: f(input) },
            &Operation::Slice { input, axis, start, end } =>
                Operation::Slice { input: f(input), axis, start, end },
            &Operation::Conv { input, filter, details: conv_shape } =>
                Operation::Conv { input: f(input), filter: f(filter), details: conv_shape },
            &Operation::Add { left, right, subtract } =>
                Operation::Add { left: f(left), right: f(right), subtract },
            &Operation::Mul { left, right } =>
                Operation::Mul { left: f(left), right: f(right) },
            &Operation::Clamp { input, min, max } =>
                Operation::Clamp { input: f(input), min, max },
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub struct ConvDetails {
    pub input_channels: usize,
    pub output_channels: usize,
    pub input_size: usize,
    pub kernel_size: usize,
    pub padding: usize,
    pub output_size: usize,
    pub batch_size: Size,
}

impl ConvDetails {
    pub fn kernel_shape(&self) -> [usize; 4] {
        [self.output_channels, self.input_channels, self.kernel_size, self.kernel_size]
    }
}

impl Index<Value> for Graph {
    type Output = ValueInfo;

    fn index(&self, value: Value) -> &Self::Output {
        self.check_contains(value);
        &self.values[value.0]
    }
}

impl Graph {
    pub fn new() -> Self {
        Graph { values: vec![], inputs: vec![], outputs: vec![] }
    }

    fn check_contains(&self, value: Value) {
        assert!(value.0 < self.values.len());
    }

    fn check_broadcast(&self, left: Value, right: Value) -> Shape {
        let left_shape = &self[left].shape;
        let right_shape = &self[right].shape;
        assert_eq!(
            left_shape.rank(), right_shape.rank(),
            "Both inputs must have the same rank, got {:?} and {:?}",
            left_shape, right_shape
        );

        for (&l, &r) in zip_eq(&left_shape.dims, &right_shape.dims) {
            assert!(l == r || r == Size::ONE, "Cannot broadcast shape {:?} to {:?}", right_shape, left_shape);
        }

        left_shape.clone()
    }

    /// Iterate over the values in this graph, in topological order,
    /// which means that nodes will only be visited after all of their inputs have been visited.
    pub fn values(&self) -> impl Iterator<Item=Value> {
        (0..self.values.len()).map(Value)
    }

    pub fn inputs(&self) -> &[Value] {
        &self.inputs
    }

    pub fn outputs(&self) -> &[Value] {
        &self.outputs
    }

    pub fn outputs_mut(&mut self) -> &mut Vec<Value> {
        &mut self.outputs
    }

    pub fn as_const(&self, value: Value) -> Option<&[f32]> {
        if let Operation::Constant { data } = &self[value].operation {
            Some(data)
        } else {
            None
        }
    }

    pub fn is_all_zero(&self, value: Value) -> bool {
        self.as_const(value).map_or(false, |x| x.iter().all(|&x| x == 0.0))
    }

    pub fn is_all_one(&self, value: Value) -> bool {
        self.as_const(value).map_or(false, |x| x.iter().all(|&x| x == 1.0))
    }

    #[must_use]
    pub(crate) fn push(&mut self, shape: Shape, operation: Operation) -> Value {
        let index = self.values.len();
        self.values.push(ValueInfo { shape, operation });
        Value(index)
    }

    /// Declare a new input value.
    #[must_use]
    pub fn input(&mut self, shape: Shape) -> Value {
        let index = self.inputs.len();
        let value = self.push(shape, Operation::Input { index });
        self.inputs.push(value);
        value
    }

    /// Declare a new constant.
    #[must_use]
    pub fn constant(&mut self, shape: Shape, data: Vec<f32>) -> Value {
        let expected_len = shape.unwrap_fixed("Constant shape must be fixed").size();
        assert_eq!(expected_len, data.len() as usize, "Shape {:?} and data size {} mismatch", shape, data.len());

        self.push(shape, Operation::Constant { data: ConstantData(data) })
    }

    /// View an existing value as a new shape.
    #[must_use]
    pub fn view(&mut self, input: Value, new_shape: Shape) -> Value {
        let old_shape = &self[input].shape;
        if &new_shape == old_shape {
            return input;
        }

        assert_eq!(
            old_shape.size(), new_shape.size(),
            "New shape {:?} must have the same size as old shape {:?}",
            new_shape, old_shape,
        );

        self.push(new_shape, Operation::View { input })
    }

    /// View a value with a flattened shape.
    /// All axis starting from `start_axis` inclusive are flattened into a single axis.
    #[must_use]
    pub fn flatten(&mut self, input: Value, start_axis: usize) -> Value {
        let old_shape = &self[input].shape;
        assert!(
            old_shape.rank() >= start_axis,
            "Input rank {} to low for start axis {}", old_shape.rank(), start_axis
        );

        let new_shape = if start_axis == 0 {
            let size = old_shape.size();
            Shape::new(vec![Size::fixed(1), size])
        } else {
            let kept_dims = &old_shape.dims[..start_axis];
            let flat_size = old_shape.dims[start_axis..].iter().copied().product();

            Shape::new([kept_dims, &[flat_size]].concat())
        };

        self.view(input, new_shape)
    }

    /// View part of an existing value.
    #[must_use]
    pub fn slice(&mut self, input: Value, axis: usize, start: usize, end: usize) -> Value {
        let old_shape = &self[input].shape;

        assert!(
            axis < old_shape.rank(),
            "Input rank {} too low for axis {}", old_shape.rank(), axis
        );

        let mut new_shape = old_shape.clone();
        let dim = new_shape[axis].unwrap_fixed_mut("Slice axis size");

        if start == 0 && end == *dim {
            return input;
        }

        assert!(
            start < *dim && end <= *dim,
            "Slice range {}..{} out of bounds for axis {} with size {}",
            start, end, axis, *dim,
        );

        *dim = end - start;
        self.push(new_shape, Operation::Slice { input, axis, start, end })
    }

    /// Index along a given axis.
    /// Similar to slice with a 1-sized interval except that the the resulting value doesn't have the extra axis.
    #[must_use]
    pub fn index(&mut self, input: Value, axis: usize, index: usize) -> Value {
        let sliced = self.slice(input, axis, index, index + 1);

        let mut new_shape = self[input].shape.clone();
        new_shape.dims.remove(axis);

        self.view(sliced, new_shape)
    }

    /// Apply 2D convolution.
    #[must_use]
    pub fn conv(&mut self, input: Value, filter: Value, padding: usize) -> Value {
        let [n, in_c, in_w, in_h]: [Size; 4] = self[input].shape.dims.as_slice().try_into()
            .expect("Convolution input must have rank 4");
        let [out_c, in_c_check, k_w, k_h]: [Size; 4] = self[filter].shape.dims.as_slice().try_into()
            .expect("Convolution filter must have rank 4");

        // almost everything must be fixed, except for the batch size n
        let in_c = in_c.unwrap_fixed("Conv input channels");
        let in_w = in_w.unwrap_fixed("Conv input width");
        let in_h = in_h.unwrap_fixed("Conv input height");
        let out_c = out_c.unwrap_fixed("Conv output channels");
        let in_c_check = in_c_check.unwrap_fixed("Filter input channels");
        let k_w = k_w.unwrap_fixed("Conv kernel width");
        let k_h = k_h.unwrap_fixed("Conv kernel height");

        assert_eq!(1, k_w % 2, "Kernel width must be odd, got {}", k_w);
        assert_eq!(1, k_h % 2, "Kernel height must be odd, got {}", k_h);

        assert_eq!(in_c, in_c_check, "Input channel mismatch");

        assert_eq!(in_w, in_h, "Only square inputs supported");
        assert_eq!(k_w, k_h, "Only square kernels supported");

        let out_w = in_w - k_w + 1 + 2 * padding;
        let out_h = in_h - k_h + 1 + 2 * padding;
        let output_shape = vec![n, Size::fixed(out_c), Size::fixed(out_w), Size::fixed(out_h)];
        let output_shape = Shape::new(output_shape);

        let details = ConvDetails {
            batch_size: n,
            input_channels: in_c,
            output_channels: out_c,
            input_size: in_w,
            kernel_size: k_w,
            padding,
            output_size: out_w,
        };
        self.push(
            output_shape,
            Operation::Conv { input, details, filter },
        )
    }

    /// Apply a linear transformation.
    /// Input shape `[N, Ci]` and weight shape `[Co, Ci]` result in an output with shape `[N, Co]`.
    #[must_use]
    pub fn linear(&mut self, input: Value, weight: Value) -> Value {
        let input_shape = self[input].shape.unwrap_2();
        let weight_shape = self[weight].shape.unwrap_2();

        let n = input_shape[0];
        let ci = input_shape[1];
        let co = weight_shape[0];
        assert_eq!(ci, weight_shape[1]);

        // convert this linear operation into the equivalent convolution
        let input_view_shape = Shape::new(vec![n, ci, Size::ONE, Size::ONE]);
        let input_view = self.view(input, input_view_shape);
        let weight_view_shape = Shape::new(vec![co, ci, Size::ONE, Size::ONE]);
        let weight_view = self.view(weight, weight_view_shape);
        let output_view_shape = Shape::new(vec![n, co]);

        let output = self.conv(input_view, weight_view, 0);
        self.view(output, output_view_shape)
    }

    /// Elementwise clamp.
    #[must_use]
    pub fn clamp(&mut self, input: Value, min: f32, max: f32) -> Value {
        if min == f32::NEG_INFINITY && max == f32::INFINITY {
            return input;
        }

        self.push(self[input].shape.clone(), Operation::Clamp { input, min, max })
    }

    /// Elementwise relu.
    #[must_use]
    pub fn relu(&mut self, input: Value) -> Value {
        self.clamp(input, 0.0, f32::INFINITY)
    }

    /// Add two values together elementwise.
    /// They must have the same rank, and the right shape is broadcasted to the left shape.
    #[must_use]
    pub fn add(&mut self, left: Value, right: Value) -> Value {
        let output_shape = self.check_broadcast(left, right);

        if self.is_all_zero(right) {
            return left;
        }

        self.push(output_shape, Operation::Add { left, right, subtract: false })
    }

    /// Subtract two values elementwise.
    /// They must have the same rank, and the right shape is broadcasted to the left shape.
    #[must_use]
    pub fn sub(&mut self, left: Value, right: Value) -> Value {
        let output_shape = self.check_broadcast(left, right);

        if self.is_all_zero(right) {
            return left;
        }

        self.push(output_shape, Operation::Add { left, right, subtract: true })
    }

    /// Multiple two values elementwise.
    /// They must have the same rank, and the right shape is broadcasted to the left shape.
    #[must_use]
    pub fn mul(&mut self, left: Value, right: Value) -> Value {
        let output_shape = self.check_broadcast(left, right);

        if self.is_all_one(right) {
            return left;
        }

        self.push(output_shape, Operation::Mul { left, right })
    }

    /// Register an existing value as an output
    pub fn output(&mut self, value: Value) {
        self.outputs.push(value);
    }

    /// Register multiple values as output at once, in order.
    pub fn output_all(&mut self, values: &[Value]) {
        for &value in values {
            self.output(value)
        }
    }
}

impl Debug for Graph {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Graph")
            .field("inputs", &self.inputs().iter().map(|&v| &self[v].shape).collect_vec())
            .field("outputs", &self.outputs().iter().map(|&v| &self[v].shape).collect_vec())
            .finish_non_exhaustive()
    }
}

impl Display for Graph {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let Graph { values, inputs, outputs } = self;

        writeln!(f, "Graph {{")?;

        writeln!(f, "  values: [")?;
        for (i, info) in values.iter().enumerate() {
            writeln!(f, "    {:?} = {:?},", Value(i), info)?;
        }
        writeln!(f, "  ],")?;

        writeln!(f, "  inputs: {:?},", inputs)?;
        writeln!(f, "  outputs: {:?},", outputs)?;

        writeln!(f, "}}")?;

        Ok(())
    }
}

impl Debug for ConstantData {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if self.0.len() <= 16 {
            write!(f, "{:?}", self.0)
        } else {
            write!(f, "[..; {}]", self.0.len())
        }
    }
}

impl Deref for ConstantData {
    type Target = Vec<f32>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}