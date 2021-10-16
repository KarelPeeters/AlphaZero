use std::borrow::Borrow;
use std::fmt::{Debug, Formatter};
use std::marker::PhantomData;

use board_game::board::Board;

use cuda_nn_eval::executor::CudnnExecutor;
use cuda_sys::wrapper::handle::{CudnnHandle, Device};
use nn_graph::graph::Graph;

use crate::mapping::BoardMapper;
use crate::network::{Network, ZeroEvaluation};
use crate::network::common::{check_graph_shapes, decode_output};

pub struct CudnnNetwork<B: Board, M: BoardMapper<B>> {
    mapper: M,
    max_batch_size: usize,

    executor: CudnnExecutor,
    graph: Graph,

    input: Vec<f32>,
    ph: PhantomData<B>,
}

impl<B: Board, M: BoardMapper<B>> CudnnNetwork<B, M> {
    pub fn new(mapper: M, graph: Graph, max_batch_size: usize, device: Device) -> Self {
        check_graph_shapes(mapper, &graph);

        let handle = CudnnHandle::new(device);
        let executor = CudnnExecutor::new(handle, &graph, max_batch_size);

        let input = vec![0.0; max_batch_size * M::INPUT_FULL_SIZE];

        CudnnNetwork { max_batch_size, mapper, graph, executor, input, ph: PhantomData }
    }
}

impl<B: Board, M: BoardMapper<B>> Network<B> for CudnnNetwork<B, M> {
    fn evaluate_batch(&mut self, boards: &[impl Borrow<B>]) -> Vec<ZeroEvaluation> {
        let batch_size = boards.len();
        let max_batch_size = self.max_batch_size;
        assert!(batch_size <= max_batch_size);

        // encode input
        self.input.clear();
        for board in boards {
            self.mapper.encode_full(&mut self.input, board.borrow())
        }

        // fill rest of input with zeros
        self.input.resize(max_batch_size * M::INPUT_FULL_SIZE, f32::NAN);

        // run the actual computation
        let outputs = self.executor.evaluate(&[&self.input]);

        // decode the relevant part of the output
        decode_output(
            self.mapper,
            boards,
            &outputs[0][0..batch_size],
            &outputs[1][0..batch_size * 3],
            &outputs[2][0..batch_size * M::POLICY_SIZE],
        )
    }
}

impl<B: Board, M: BoardMapper<B>> Debug for CudnnNetwork<B, M> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CudnnNetwork")
            .field("mapper", &self.mapper)
            .field("graph", &self.graph)
            .field("max_batch_size", &self.max_batch_size)
            .finish()
    }
}
