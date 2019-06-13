// Copyright 2019 The Exonum Team
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// This is a regression test for exonum node.

use futures::{sync::oneshot, Future, IntoFuture};
use serde_json::Value;
use tokio::util::FutureExt;
use tokio_core::reactor::Core;

use std::{
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

use exonum_merkledb::{Database, Fork, Snapshot, TemporaryDB};

use exonum::{
    blockchain::{Service, ServiceContext, Transaction},
    crypto::Hash,
    helpers,
    messages::AnyTx,
    node::{ApiSender, ExternalMessage, Node},
};

struct CommitWatcherService(pub Mutex<Option<oneshot::Sender<()>>>);

impl Service for CommitWatcherService {
    fn service_id(&self) -> u16 {
        255
    }

    fn service_name(&self) -> &str {
        "commit_watcher"
    }

    fn state_hash(&self, _: &dyn Snapshot) -> Vec<Hash> {
        Vec::new()
    }

    fn tx_from_raw(&self, _raw: AnyTx) -> Result<Box<dyn Transaction>, failure::Error> {
        unreachable!("An unknown transaction received");
    }

    fn after_commit(&self, _context: &ServiceContext) {
        if let Some(oneshot) = self.0.lock().unwrap().take() {
            oneshot.send(()).unwrap();
        }
    }
}

struct InitializeCheckerService(pub Arc<Mutex<u64>>);

impl Service for InitializeCheckerService {
    fn service_id(&self) -> u16 {
        256
    }

    fn service_name(&self) -> &str {
        "initialize_checker"
    }

    fn state_hash(&self, _: &dyn Snapshot) -> Vec<Hash> {
        Vec::new()
    }

    fn tx_from_raw(&self, _raw: AnyTx) -> Result<Box<dyn Transaction>, failure::Error> {
        unreachable!("An unknown transaction received");
    }

    fn initialize(&self, _fork: &Fork) -> Value {
        *self.0.lock().unwrap() += 1;
        Value::Null
    }
}

struct RunHandle {
    node_thread: JoinHandle<()>,
    api_tx: ApiSender,
}

fn run_nodes(count: u16, start_port: u16) -> (Vec<RunHandle>, Vec<oneshot::Receiver<()>>) {
    let mut node_threads = Vec::new();
    let mut commit_rxs = Vec::new();
    for node_cfg in helpers::generate_testnet_config(count, start_port) {
        let (commit_tx, commit_rx) = oneshot::channel();
        //        let service = Box::new(CommitWatcherService(Mutex::new(Some(commit_tx))));
        let node = Node::new(TemporaryDB::new(), Vec::new(), node_cfg, None);
        let api_tx = node.channel();
        node_threads.push(RunHandle {
            node_thread: thread::spawn(move || {
                node.run().unwrap();
            }),
            api_tx,
        });
        commit_rxs.push(commit_rx);
    }
    (node_threads, commit_rxs)
}

#[test]
#[ignore = "TODO: Research why node tests randomly fails. [ECR-2363]"]
fn test_node_run() {
    let (nodes, commit_rxs) = run_nodes(4, 16_300);

    let mut core = Core::new().unwrap();
    let duration = Duration::from_secs(60);
    for rx in commit_rxs {
        let future = rx.into_future().timeout(duration).map_err(drop);
        core.run(future).expect("failed commit");
    }

    for handle in nodes {
        handle
            .api_tx
            .send_external_message(ExternalMessage::Shutdown)
            .unwrap();
        handle.node_thread.join().unwrap();
    }
}

#[test]
#[ignore = "TODO restore dispatcher state after node restart [ECR-3276]"]
fn test_node_restart_regression() {
    let start_node = |node_cfg, db, init_times| {
        //        let service = Box::new(InitializeCheckerService(init_times));
        // TODO: use new service API.
        let node = Node::new(db, Vec::new(), node_cfg, None);
        let api_tx = node.channel();
        let node_thread = thread::spawn(move || {
            node.run().unwrap();
        });
        // Wait for shutdown
        api_tx
            .send_external_message(ExternalMessage::Shutdown)
            .unwrap();
        node_thread.join().unwrap();
    };

    let db = Arc::from(Box::new(TemporaryDB::new()) as Box<dyn Database>) as Arc<dyn Database>;
    let node_cfg = helpers::generate_testnet_config(1, 3600)[0].clone();

    let init_times = Arc::new(Mutex::new(0));
    // First launch
    start_node(node_cfg.clone(), db.clone(), Arc::clone(&init_times));
    // Second launch
    start_node(node_cfg, db, Arc::clone(&init_times));
    assert_eq!(*init_times.lock().unwrap(), 1);
}
