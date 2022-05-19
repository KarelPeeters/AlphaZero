use itertools::Itertools;

use cuda_nn_eval::device_tensor::DeviceTensor;
use cuda_sys::wrapper::handle::Device;
use nn_graph::graph::{BinaryOp, Graph, ReduceOp, SliceRange, UnaryOp, Value};
use nn_graph::ndarray::Array1;
use nn_graph::shape;
use nn_graph::shape::{Shape, Size};

use crate::root::runner::{test_all, test_all_graph};
use crate::root::tensor_utils::{linspace_tensor, linspace_vec, manual_tensor, range_vec};

#[test]
fn empty() {
    test_all(&Graph::new(), 8, &[], Some(&[]))
}

#[test]
fn copy() {
    let mut graph = Graph::new();

    let fixed_size = 10;
    let batch_size = 8;

    let fixed = graph.input(shape![fixed_size]);
    let batch = graph.input(shape![Size::BATCH]);
    graph.output_all(&[fixed, batch]);

    let fixed_tensor = linspace_tensor(fixed_size).into_dyn();
    let batch_tensor = linspace_tensor(batch_size).into_dyn();

    test_all(
        &graph,
        batch_size,
        &[fixed_tensor.to_shared(), batch_tensor.to_shared()],
        Some(&[fixed_tensor, batch_tensor]),
    )
}

#[test]
fn slice() {
    let mut graph = Graph::new();

    let input = graph.input(shape![10, 4]);
    let input_tensor = linspace_tensor((10, 4));

    let indexed = graph.index(input, 1, 0);
    let outputs = [
        // start:end slicing
        indexed,
        graph.slice(input, 0, SliceRange::new(0, 2, 1)),
        graph.slice(indexed, 0, SliceRange::new(0, 2, 1)),
    ];

    graph.output_all(&outputs);

    test_all(&graph, 0, &[input_tensor.into_dyn()], None)
}

#[test]
fn flip() {
    let mut graph = Graph::new();

    let x = graph.constant(shape![2, 3].clone(), vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
    let first_once = graph.flip(x, 0);
    let first_twice = graph.flip(first_once, 0);

    let other_once = graph.flip(x, 1);
    let other_twice = graph.flip(other_once, 1);

    let combined = graph.flip(first_once, 1);

    graph.output_all(&[first_once, first_twice, other_once, other_twice, combined]);

    test_all(
        &graph,
        0,
        &[],
        Some(&[
            manual_tensor((2, 3), vec![3.0, 4.0, 5.0, 0.0, 1.0, 2.0]),
            manual_tensor((2, 3), vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0]),
            manual_tensor((2, 3), vec![2.0, 1.0, 0.0, 5.0, 4.0, 3.0]),
            manual_tensor((2, 3), vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0]),
            manual_tensor((2, 3), vec![5.0, 4.0, 3.0, 2.0, 1.0, 0.0]),
        ]),
    );
}

#[test]
fn flip_conv() {
    let mut graph = Graph::new();

    let input = graph.input(shape![2, 4, 8, 8]);
    let flipped = graph.flip(input, 3);

    let weight = graph.constant(shape![4, 4, 3, 3], linspace_vec(4 * 4 * 3 * 3));
    let result = graph.conv(flipped, weight, 1, 1);

    graph.output(result);
    test_all(&graph, 0, &[linspace_tensor((2, 4, 8, 8)).into_dyn()], None);
}

#[test]
fn repeat() {
    let tensor = DeviceTensor::alloc_simple(Device::new(0), vec![0]);
    println!("{:?}", unsafe { tensor.ptr().ptr() });

    let mut graph = Graph::new();

    let x = graph.constant(shape![2, 3], linspace_vec(6));

    let outputs = [
        graph.repeat(x, 0, 0),
        graph.repeat(x, 0, 2),
        graph.repeat(x, 1, 0),
        graph.repeat(x, 1, 2),
    ];

    graph.output_all(&outputs);

    test_all(&graph, 0, &[], None);
}

#[test]
fn gather() {
    let mut graph = Graph::new();

    let input = graph.constant(shape![2, 3], range_vec(2 * 3 * 1));
    let index = graph.constant(shape![4], vec![0.0, 2.0, 1.0, 0.0]);
    let result = graph.gather(input, 1, index);
    graph.output(result);

    let expected_result = manual_tensor((2, 4), vec![0.0, 2.0, 1.0, 0.0, 3.0, 5.0, 4.0, 3.0]);

    test_all(&graph, 0, &[], Some(&[expected_result]))
}

#[test]
fn linear() {
    let mut graph = Graph::new();

    let input = graph.input(shape![1, 4]);
    let weight = graph.constant(shape![2, 4], range_vec(8));
    let bias = graph.constant(shape![1, 2], vec![-10.0, 10.0]);

    let linear = graph.linear(input, weight);
    let biased = graph.add(linear, bias);

    graph.output_all(&[linear, biased]);

    test_all(
        &graph,
        0,
        &[manual_tensor((1, 4), vec![0.0, 1.0, 2.0, 3.0])],
        Some(&[
            manual_tensor((1, 2), vec![14.0, 38.0]),
            manual_tensor((1, 2), vec![4.0, 48.0]),
        ]),
    )
}

#[test]
fn linear_sliced() {
    let mut graph = Graph::new();

    let left = graph.input(shape![8, 4]);
    let left_sliced = graph.slice(left, 0, SliceRange::new(0, 8, 2));
    let right = graph.input(shape![4, 3]);

    let result = graph.mat_mul(left_sliced, right);
    graph.output(result);

    test_all(
        &graph,
        0,
        &[linspace_tensor((8, 4)).into_dyn(), linspace_tensor((4, 3)).into_dyn()],
        None,
    );
}

#[test]
fn mat_mul() {
    // run the "transposed" cases first since they're simpler for cublas
    for transpose_a in [true, false] {
        for transpose_b in [true, false] {
            println!("Transpose a: {}, b: {}", transpose_a, transpose_b);
            let mut graph = Graph::new();

            let mut shape_a = shape![4, 5, 6];
            let mut shape_b = shape![4, 6, 3];

            if transpose_a {
                shape_a.dims.swap(1, 2);
            }
            if transpose_b {
                shape_b.dims.swap(1, 2);
            }

            let a_orig = graph.constant(shape_a, linspace_vec(4 * 5 * 6));
            let b_orig = graph.constant(shape_b, linspace_vec(4 * 6 * 3));

            let a = if transpose_a {
                graph.permute(a_orig, vec![0, 2, 1])
            } else {
                a_orig
            };
            let b = if transpose_b {
                graph.permute(b_orig, vec![0, 2, 1])
            } else {
                b_orig
            };

            let result = graph.batched_mat_mul(a, b);
            assert_eq!(graph[result].shape, shape![4, 5, 3]);
            graph.output(result);

            test_all(&graph, 0, &[], None);
        }
    }
}

#[test]
fn horizontal_1x1_conv() {
    let mut graph = Graph::new();

    let input = graph.constant(shape![2, 4, 1, 8], linspace_vec(2 * 4 * 8));
    let filter = graph.constant(shape![3, 4, 1, 1], linspace_vec(3 * 4));

    let output = graph.conv(input, filter, 0, 0);
    graph.output(output);

    assert_eq!(graph[output].shape, shape![2, 3, 1, 8]);
    test_all(&graph, 0, &[], None)
}

#[test]
fn vertical_1x1_conv() {
    let mut graph = Graph::new();

    let input = graph.constant(shape![2, 4, 8, 1], linspace_vec(2 * 4 * 8));
    let filter = graph.constant(shape![3, 4, 1, 1], linspace_vec(3 * 4));

    let output = graph.conv(input, filter, 0, 0);
    graph.output(output);

    assert_eq!(graph[output].shape, shape![2, 3, 8, 1]);
    test_all(&graph, 0, &[], None)
}

#[test]
fn fuse_clamp() {
    let mut graph = Graph::new();

    let mut curr = graph.input(shape![Size::BATCH]);

    curr = graph.clamp(curr, -5.0, f32::INFINITY);
    curr = graph.clamp(curr, f32::NEG_INFINITY, 2.0);
    curr = graph.clamp(curr, 0.0, 1.0);
    curr = graph.clamp(curr, -1.0, 2.0);

    graph.output(curr);

    test_all(
        &graph,
        5,
        &[manual_tensor(5, vec![-2.0, 0.0, 0.5, 1.0, 2.0])],
        Some(&[manual_tensor(5, vec![0.0, 0.0, 0.5, 1.0, 1.0])]),
    )
}

#[test]
fn ele_broadcast() {
    // don't test division, since the GPU doesn't support it yet
    for op in [
        BinaryOp::Add,
        BinaryOp::Sub,
        BinaryOp::Mul,
        BinaryOp::Min,
        BinaryOp::Max,
    ] {
        println!("Testing operation {:?}", op);

        let mut graph = Graph::new();
        let left = graph.constant(shape![2, 3, 4], linspace_vec(2 * 3 * 4));

        for shape in [Shape::SCALAR, shape![1, 1, 1], shape![2, 3, 4], shape![2, 1, 4]] {
            println!("  with right shape {}", shape);
            let size = shape.size().eval(0);
            let right = graph.constant(shape, linspace_vec(size));
            let result = graph.binary(op, left, right);
            graph.output(result);
        }

        test_all(&graph, 0, &[], None);
    }
}

#[test]
fn add_broadcast() {
    let mut graph = Graph::new();

    let left = graph.constant(shape![2, 2, 2], vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
    let right = graph.constant(shape![1, 2, 2], vec![0.0, 1.0, 2.0, 3.0]);
    let output = graph.add(left, right);
    graph.output(output);

    test_all(
        &graph,
        0,
        &[],
        Some(&[manual_tensor((2, 2, 2), vec![0.0, 2.0, 4.0, 6.0, 4.0, 6.0, 8.0, 10.0])]),
    )
}

#[test]
fn affine_single_element() {
    let input_data = manual_tensor((8, 1, 1, 1), range_vec(8));
    let output_data = input_data.map(|&x| ((x + 1.0) * 2.0 * 10.0 + 3.0) * 4.0).to_shared();

    let mut graph = Graph::new();

    let const_shape = shape![1, 1, 1, 1];
    let bias_0 = graph.constant(const_shape.clone(), vec![1.0]);
    let scale_0 = graph.constant(const_shape.clone(), vec![2.0]);
    let filter = graph.constant(const_shape.clone(), vec![10.0]);
    let bias_1 = graph.constant(const_shape.clone(), vec![3.0]);
    let scale_1 = graph.constant(const_shape.clone(), vec![4.0]);

    let curr = graph.input(Shape::fixed(input_data.shape()));
    let curr = graph.add(curr, bias_0);
    let curr = graph.mul(curr, scale_0);
    let curr = graph.conv(curr, filter, 0, 0);
    let curr = graph.add(curr, bias_1);
    let curr = graph.mul(curr, scale_1);
    graph.output(curr);

    test_all(&graph, 0, &[input_data], Some(&[output_data]))
}

#[test]
fn affine_add_twice() {
    let mut graph = Graph::new();

    let x = graph.input(shape![Size::BATCH, 1, 1, 1]);
    let w1 = graph.constant(shape![1, 1, 1, 1], vec![1.0]);
    let w2 = graph.constant(shape![1, 1, 1, 1], vec![2.0]);

    let y1 = graph.add(x, w1);
    let y2 = graph.add(y1, w2);

    graph.output(y2);

    test_all(
        &graph,
        2,
        &[manual_tensor((2, 1, 1, 1), vec![0.0, 1.0])],
        Some(&[manual_tensor((2, 1, 1, 1), vec![3.0, 4.0])]),
    )
}

#[test]
fn affine_single_div() {
    let mut graph = Graph::new();

    let left = graph.constant(shape![2, 3], range_vec(2 * 3));
    let right = graph.constant(Shape::SCALAR, vec![2.0]);
    let result = graph.binary(BinaryOp::Div, left, right);
    graph.output(result);

    let expected = vec![0.0, 0.5, 1.0, 1.5, 2.0, 2.5];
    test_all_graph(&graph, 0, &[], Some(&[manual_tensor((2, 3), expected)]));
}

#[test]
fn affine_multiple_channels() {
    let input_data = manual_tensor((8, 3, 1, 1), range_vec(8 * 3));

    let mut graph = Graph::new();

    let before_shape = shape![1, 3, 1, 1];
    let after_shape = shape![1, 2, 1, 1];
    let filter_shape = shape![2, 3, 1, 1];

    let bias_0 = graph.constant(before_shape.clone(), vec![1.0, 2.0, 3.0]);
    let scale_0 = graph.constant(before_shape.clone(), vec![2.0, 3.0, 4.0]);
    let filter = graph.constant(filter_shape.clone(), vec![10.0, 20.0, 30.0, 40.0, 50.0, 60.0]);
    let bias_1 = graph.constant(after_shape.clone(), vec![3.0, 4.0]);
    let scale_1 = graph.constant(after_shape.clone(), vec![4.0, 5.0]);

    let curr = graph.input(Shape::fixed(input_data.shape()));
    let curr = graph.add(curr, bias_0);
    let curr = graph.mul(curr, scale_0);
    let curr = graph.conv(curr, filter, 0, 0);
    let curr = graph.add(curr, bias_1);
    let curr = graph.mul(curr, scale_1);
    graph.output(curr);

    test_all(&graph, 0, &[input_data], None)
}

#[test]
fn affine_padding() {
    let input_data = linspace_tensor((8, 3, 8, 8)).into_dyn();
    let filter_data = linspace_tensor((5, 3, 3, 3));

    let mut graph = Graph::new();

    let filter = graph.constant(Shape::fixed(filter_data.shape()), filter_data.to_owned().into_raw_vec());
    let bias_0 = graph.constant(shape![1, 3, 1, 1], linspace_vec(3));
    let bias_1 = graph.constant(shape![1, 5, 1, 1], linspace_vec(5));

    let mut curr = graph.input(Shape::fixed(&input_data.shape()));
    curr = graph.add(curr, bias_0);
    curr = graph.conv(curr, filter, 1, 1);
    curr = graph.add(curr, bias_1);
    graph.output(curr);

    test_all(&graph, 0, &[input_data], None)
}

#[test]
fn pre_act_resnet() {
    let mut graph = Graph::new();

    let input_data = linspace_tensor((8, 3, 8, 8)).into_dyn();
    let input = graph.input(Shape::fixed(input_data.shape()));

    let filter_initial = graph.constant(shape![5, 3, 3, 3], linspace_vec(5 * 3 * 3 * 3));
    let filter_tower = graph.constant(shape![5, 5, 3, 3], linspace_vec(5 * 5 * 3 * 3));
    let filter_policy = graph.constant(shape![2, 5, 1, 1], linspace_vec(5 * 2));

    let mut tower = graph.conv(input, filter_initial, 1, 1);
    for _ in 0..2 {
        let mut curr = channel_batchnorm(&mut graph, tower);
        curr = graph.clamp(curr, 0.0, 6.0);
        curr = graph.conv(curr, filter_tower, 1, 1);
        curr = graph.clamp(curr, 0.0, 6.0);
        curr = graph.conv(curr, filter_tower, 1, 1);
        tower = graph.add(curr, tower);
    }

    let policy = graph.conv(tower, filter_policy, 0, 0);

    graph.output(tower);
    graph.output(policy);

    test_all(&graph, 0, &[input_data], None)
}

fn channel_batchnorm(graph: &mut Graph, input: Value) -> Value {
    let [_, c, _, _] = graph[input].shape.unwrap_4();
    let c = c.unwrap_fixed("Dummy BN channel count");

    let const_shape = shape![1, c, 1, 1];

    let mean = graph.constant(const_shape.clone(), linspace_vec(c));
    let var = graph.constant(const_shape.clone(), linspace_vec(c));
    let scale = graph.constant(const_shape.clone(), linspace_vec(c));
    let bias = graph.constant(const_shape.clone(), linspace_vec(c));

    let mut curr = input;
    curr = graph.add(curr, mean);
    curr = graph.mul(curr, var);
    curr = graph.mul(curr, scale);
    curr = graph.add(curr, bias);
    curr
}

#[test]
fn fuse_res() {
    let mut graph = Graph::new();

    let input = graph.input(shape![10, 4, 8, 8]);
    let other = graph.input(shape![10, 4, 8, 8]);
    let filter = graph.constant(shape![4, 4, 3, 3], linspace_vec(4 * 4 * 3 * 3));

    let mut curr = input;
    curr = graph.conv(curr, filter, 1, 1);
    curr = graph.add(curr, other);
    curr = graph.clamp(curr, 0.0, f32::INFINITY);
    graph.output(curr);

    test_all(
        &graph,
        0,
        &[
            linspace_tensor((10, 4, 8, 8)).into_dyn(),
            linspace_tensor((10, 4, 8, 8)).into_dyn() + 1.0,
        ],
        None,
    );
}

#[test]
fn concat() {
    let mut graph = Graph::new();

    let a = graph.constant(shape![2, 3, 4], linspace_vec(2 * 3 * 4));
    let b = graph.constant(shape![2, 1, 4], linspace_vec(2 * 1 * 4));
    let c = graph.constant(shape![2, 8, 4], linspace_vec(2 * 8 * 4));

    let result = graph.concat(vec![a, b, c], 1, None);
    graph.output(result);

    test_all(&graph, 0, &[], None);
}

#[test]
fn permute() {
    let mut graph = Graph::new();

    let a = graph.constant(shape![2, 3, 4, 5], range_vec(2 * 3 * 4 * 5));
    for (i, permutation) in (0..4).permutations(4).enumerate() {
        println!("Output {} is permutation {:?}", i, permutation);

        let result = graph.permute(a, permutation);
        graph.output(result);
    }

    test_all(&graph, 0, &[], None);
}

#[test]
fn chain() {
    // child implements y = x * 2.0
    let mut child = Graph::new();
    {
        let child_x = child.input(shape![2]);
        let child_w = child.constant(shape![1], vec![2.0]);
        let child_y = child.mul(child_x, child_w);
        child.output(child_y);
    }

    // parent implements y = child(x + 3.0)
    let mut parent = Graph::new();
    let parent_x = parent.input(shape![2]);
    let parent_w = parent.constant(shape![1], vec![3.0]);
    let parent_z = parent.add(parent_x, parent_w);
    let parent_y = parent.call(&child, &[parent_z]);

    assert_eq!(parent_y.len(), 1);
    parent.output(parent_y[0]);

    test_all(
        &parent,
        0,
        &[manual_tensor(2, vec![1.0, 2.0])],
        Some(&[manual_tensor(2, vec![8.0, 10.0])]),
    )
}

#[test]
fn repeated_conv() {
    let mut graph = Graph::new();

    // weights must be different, otherwise the graph builder already deduplicates nodes
    let weight0 = graph.constant(shape![4, 4, 3, 3], Array1::linspace(-1.0, 1.0, 4 * 4 * 3 * 3).to_vec());
    let weight1 = graph.constant(shape![4, 4, 3, 3], Array1::linspace(-2.0, 2.0, 4 * 4 * 3 * 3).to_vec());

    let input = graph.input(shape!(Size::BATCH, 4, 8, 8));

    let x1 = graph.conv(input, weight0, 1, 1);
    let x2 = graph.conv(x1, weight0, 1, 1);
    let x3 = graph.conv(x2, weight0, 1, 1);
    let x4 = graph.conv(x3, weight0, 1, 1);

    let y1 = graph.conv(input, weight1, 1, 1);
    let y2 = graph.conv(y1, weight1, 1, 1);
    let y3 = graph.conv(y2, weight1, 1, 1);
    let y4 = graph.conv(y3, weight1, 1, 1);

    graph.output_all(&[x4, y4]);

    test_all(&graph, 2, &[linspace_tensor((2, 4, 8, 8)).into_dyn()], None);
}

#[test]
fn softmax() {
    let mut graph = Graph::new();

    let input = graph.input(shape![3, 3]);

    let result0 = graph.softmax(input, 0);
    let result1 = graph.softmax(input, 1);

    graph.output_all(&[result0, result1]);

    let input_data = vec![0.0, 0.0, 0.0, 1.0, 2.0, 3.0, 1.0, -1.0, f32::NEG_INFINITY];
    test_all(&graph, 0, &[manual_tensor((3, 3), input_data)], None);
}

#[test]
fn reduce_easy() {
    let mut graph = Graph::new();

    let input = graph.input(shape![4, 3]);

    for &axis in &[0, 1] {
        for &op in ReduceOp::ALL {
            let result = graph.reduce(input, vec![axis], op);
            graph.output(result);
        }
    }

    let input_data = vec![0.0, 0.0, 0.0, 1.0, 2.0, 3.0, 1.0, -1.0, -1.0 / 0.0, 0.0, 1.0, 2.0];
    test_all(&graph, 0, &[manual_tensor((4, 3), input_data)], None);
}

#[test]
fn reduce_mixed() {
    let mut graph = Graph::new();

    let input = graph.input(shape![12, 3, 7, 9, 13]);
    let mixed = graph.permute(input, vec![0, 3, 2, 1, 4]);
    let output = graph.reduce(mixed, vec![1, 2, 4], ReduceOp::Sum);
    graph.output(output);

    test_all(&graph, 0, &[linspace_tensor((12, 3, 7, 9, 13)).into_dyn()], None);
}

#[test]
fn reduce_single() {
    let mut graph = Graph::new();

    let input = graph.input(shape![4]);
    let output = graph.reduce(input, vec![0], ReduceOp::Sum);
    graph.output(output);

    test_all(&graph, 0, &[linspace_tensor(4).into_dyn()], None);
}

#[test]
fn softmax_single() {
    let mut graph = Graph::new();

    let input = graph.input(shape![4]);
    let output = graph.softmax(input, 0);
    graph.output(output);

    test_all(&graph, 0, &[linspace_tensor(4).into_dyn()], None);
}

#[test]
fn layernorm_fused() {
    let mut graph = Graph::new();

    let input = graph.input(shape![Size::BATCH, 8, 32]);
    let reduced_shape = shape![Size::BATCH, 8, 1];

    let const_2 = graph.constant(Shape::SCALAR, vec![2.0]);
    let const_eps = graph.constant(Shape::SCALAR, vec![1e-5]);

    let mean = graph.reduce(input, vec![2], ReduceOp::Mean);
    let mean = graph.view(mean, reduced_shape.clone());
    let zeroed = graph.sub(input, mean);

    let pow = graph.pow(zeroed, const_2);
    let var = graph.reduce(pow, vec![2], ReduceOp::Mean);
    let var = graph.view(var, reduced_shape.clone());
    let var = graph.add(var, const_eps);

    let std = graph.unary(UnaryOp::Sqrt, var);
    let result = graph.binary(BinaryOp::Div, zeroed, std);

    graph.output(result);

    test_all(&graph, 2, &[linspace_tensor((2, 8, 32)).into_dyn()], None);
}
