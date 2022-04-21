use crossbeam::channel::Receiver as CReceiver;
use crossbeam::channel::Sender as CSender;
use std::future::Future;
use std::iter::zip;
use std::time::Duration;
use tokio::sync::oneshot::Sender as TSender;

use futures::FutureExt;

#[derive(Clone)]
struct GpuClient {
    job_sender: CSender<(u32, TSender<u32>)>,
}

impl GpuClient {
    fn map(&self, x: u32) -> impl Future<Output = u32> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.job_sender.send((x, sender)).unwrap();
        receiver.map(|r| r.unwrap())
    }
}

fn gpu_main(job_receiver: CReceiver<(u32, TSender<u32>)>, batch_size: usize) {
    loop {
        // collect jobs
        let mut batch_x = vec![];
        let mut batch_senders = vec![];
        while batch_x.len() < batch_size {
            let (x, sender) = job_receiver.recv().unwrap();
            batch_x.push(x);
            batch_senders.push(sender);
        }

        // process entire batch at once (simulate some blocking that takes a while)
        println!("Handling batch {:?}", batch_x);
        let batch_y: Vec<_> = batch_x.into_iter().map(|x| x + 1).collect();
        std::thread::sleep(Duration::from_secs(1));

        // respond to requests
        for (sender, y) in zip(batch_senders, batch_y) {
            sender.send(y).unwrap();
        }
    }
}

fn main() {
    let batch_size = 128;
    let future_count = 256;
    let channel_capacity = future_count;

    let (job_sender, job_receiver) = crossbeam::channel::bounded(channel_capacity);
    let client = GpuClient { job_sender };
    let h = std::thread::spawn(move || {
        gpu_main(job_receiver, batch_size);
    });

    let runtime = tokio::runtime::Builder::new_multi_thread().build().unwrap();

    for fi in 0..future_count {
        let client = client.clone();
        runtime.spawn(async move {
            for x in 0.. {
                let before = std::thread::current().id();
                let y = client.map(x).await;
                let after = std::thread::current().id();
                println!(
                    "Future {} mapped {} to {}, threads {:?} and {:?}",
                    fi, x, y, before, after
                );
            }
        });
    }

    h.join().unwrap();
}
