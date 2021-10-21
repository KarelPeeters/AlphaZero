use nn_graph::graph::Graph;
use nn_graph::ndarray::s;
use nn_graph::shape::{Shape, Size};

use crate::root::runner::test_all;
use crate::root::tensor_utils::{linspace_tensor, manual_tensor, range_vec};

#[test]
fn empty() {
    test_all(&Graph::new(), 8, &[], Some(&[]))
}

#[test]
fn copy() {
    let mut graph = Graph::new();

    let fixed_size = 10;
    let batch_size = 8;

    let fixed = graph.input(Shape::fixed(&[fixed_size]));
    let batch = graph.input(Shape::new(vec![Size::BATCH]));
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

    let input = graph.input(Shape::fixed(&[10, 4]));
    let indexed = graph.index(input, 1, 0);
    let sliced = graph.slice(input, 0, 0, 2);
    let both = graph.slice(indexed, 0, 0, 2);
    graph.output_all(&[indexed, sliced, both]);

    let input_tensor = linspace_tensor((10, 4));

    test_all(
        &graph,
        0,
        &[input_tensor.to_shared().into_dyn()],
        Some(&[
            input_tensor.slice(s![.., 0]).into_dyn().to_shared(),
            input_tensor.slice(s![0..2, ..]).into_dyn().to_shared(),
            input_tensor.slice(s![0..2, 0]).into_dyn().to_shared(),
        ]),
    )
}

#[test]
fn linear() {
    let mut graph = Graph::new();

    let input = graph.input(Shape::fixed(&[1, 4]));
    let weight = graph.constant(Shape::fixed(&[2, 4]), range_vec(8));
    let bias = graph.constant(Shape::fixed(&[1, 2]), vec![-10.0, 10.0]);

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
fn fuse_clamp() {
    let mut graph = Graph::new();

    let mut curr = graph.input(Shape::new(vec![Size::BATCH]));

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
