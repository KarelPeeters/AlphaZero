use bytemuck::cast_slice_mut;
use itertools::{Itertools, zip_eq};

use cuda_sys::wrapper::handle::{CudnnHandle, Device};
use nn_graph::cpu::Tensor;
use nn_graph::graph::{Graph, Value};
use nn_graph::ndarray::{Dimension, IxDyn};

use crate::executor::CudnnExecutor;

pub const CHECK_BATCH_SIZE: usize = 2;

/// Check that the given graph produces the correct outputs as described by `check_data`,
/// which typically comes from a `.bin` file next to the `.onnx` file.
pub fn check_cudnn(graph: &Graph, check_data_bytes: &[u8]) {
    let (inputs, expected_outputs) = load_check_data(graph, check_data_bytes);
    let outputs = eval_cudnn(&graph, CHECK_BATCH_SIZE, &inputs);
    assert_outputs_match(&expected_outputs, &outputs, false);
}

const ERROR_TOLERANCE: f32 = 0.0001;

pub fn assert_outputs_match(expected_outputs: &[Tensor], outputs: &[Tensor], print: bool) {
    assert_eq!(expected_outputs.len(), outputs.len(), "Wrong number of outputs");

    let mut max_error = 0.0;

    for (i, (expected_output, output)) in zip_eq(expected_outputs, outputs).enumerate() {
        assert_eq!(expected_output.shape(), output.shape(), "Wrong output shape for output {}", i);

        for ((indices, &expected_value), &value) in zip_eq(expected_output.indexed_iter(), output.iter()) {
            let error = (expected_value - value).abs();
            max_error = f32::max(max_error, error);
            assert!(
                error < ERROR_TOLERANCE,
                "Wrong output value {}, expected {} at indices {:?} in output {}",
                value, expected_value, indices.slice(), i,
            )
        }

        if print {
            println!("Output {} matched, max error {}", i, max_error);
        }
    }
}

pub fn eval_cudnn(graph: &Graph, batch_size: usize, inputs: &[Tensor]) -> Vec<Tensor> {
    let inputs = inputs.iter()
        .map(|x| x.as_slice().expect("Only sliceable inputs supported in test framework"))
        .collect_vec();

    let handle = CudnnHandle::new(Device::new(0));
    let mut executor = CudnnExecutor::new(handle, graph, batch_size);
    let gpu_outputs = executor.evaluate(&inputs);

    // turn into Tensors, using the cpu shapes
    let outputs = zip_eq(graph.outputs(), gpu_outputs)
        .map(|(&value, output)| {
            let shape = graph[value].shape.eval(batch_size);
            Tensor::from_shape_vec(&*shape.dims, output.clone())
                .expect("GPU output has wrong length")
        })
        .collect_vec();

    outputs
}

/// Load the check data into `(inputs, expected_outputs)`.
pub fn load_check_data(graph: &Graph, check_data_bytes: &[u8]) -> (Vec<Tensor>, Vec<Tensor>) {
    assert_eq!(
        check_data_bytes.len() % 4, 0,
        "Data byte count must be multiple of 4 to be able to cast to float, got {}",
        check_data_bytes.len()
    );

    // copy the data into a float array instead of just casting it to ensure it's properly aligned
    let mut check_data = vec![0.0; check_data_bytes.len() / 4];
    cast_slice_mut(&mut check_data).copy_from_slice(check_data_bytes);

    let mut buf = &*check_data;
    let inputs = load_check_values(graph, &mut buf, graph.inputs());
    let expected_outputs = load_check_values(graph, &mut buf, graph.outputs());

    assert!(buf.is_empty(), "Leftover elements in check data buffer: {}", buf.len());

    (inputs, expected_outputs)
}

/// Load the given values from the buffer while advancing it.
fn load_check_values(graph: &Graph, buf: &mut &[f32], values: &[Value]) -> Vec<Tensor> {
    values.iter()
        .map(|&value| {
            let shape = graph[value].shape.eval(CHECK_BATCH_SIZE);
            let tensor = Tensor::from_shape_vec(
                IxDyn(&shape.dims),
                buf[0..shape.size()].to_vec(),
            ).unwrap();
            *buf = &buf[shape.size()..];
            tensor
        })
        .collect_vec()
}
