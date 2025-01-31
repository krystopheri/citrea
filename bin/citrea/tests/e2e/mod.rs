use std::fs;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use citrea_evm::smart_contracts::SimpleStorageContract;
use citrea_evm::system_contracts::L1BlockHashList;
use citrea_stf::genesis_config::GenesisPaths;
use ethereum_types::H256;
use ethers::abi::Address;
use reth_primitives::{BlockNumberOrTag, TxHash};
use sov_mock_da::{MockAddress, MockDaService, MockDaSpec, MockHash};
use sov_modules_stf_blueprint::kernels::basic::BasicKernelGenesisPaths;
use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::rpc::SoftConfirmationStatus;
use sov_rollup_interface::services::da::DaService;
use sov_stf_runner::RollupProverConfig;
use tokio::task::JoinHandle;
use tokio::time::sleep;

use crate::evm::{init_test_rollup, make_test_client};
use crate::test_client::TestClient;
use crate::test_helpers::{start_rollup, NodeMode};
use crate::DEFAULT_MIN_SOFT_CONFIRMATIONS_PER_COMMITMENT;

struct TestConfig {
    seq_min_soft_confirmations: u64,
}

impl Default for TestConfig {
    fn default() -> Self {
        Self {
            seq_min_soft_confirmations: DEFAULT_MIN_SOFT_CONFIRMATIONS_PER_COMMITMENT,
        }
    }
}

async fn initialize_test(
    config: TestConfig,
) -> (
    Box<TestClient>, /* seq_test_client */
    Box<TestClient>, /* full_node_test_client */
    JoinHandle<()>,  /* seq_task */
    JoinHandle<()>,  /* full_node_task */
    Address,
) {
    let (seq_port_tx, seq_port_rx) = tokio::sync::oneshot::channel();

    let seq_task = tokio::spawn(async move {
        start_rollup(
            seq_port_tx,
            GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
            BasicKernelGenesisPaths {
                chain_state: "../test-data/genesis/integration-tests/chain_state.json".into(),
            },
            RollupProverConfig::Execute,
            NodeMode::SequencerNode,
            None,
            config.seq_min_soft_confirmations,
            true,
        )
        .await;
    });

    let seq_port = seq_port_rx.await.unwrap();
    let seq_test_client = make_test_client(seq_port).await;

    let (full_node_port_tx, full_node_port_rx) = tokio::sync::oneshot::channel();

    let full_node_task = tokio::spawn(async move {
        start_rollup(
            full_node_port_tx,
            GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
            BasicKernelGenesisPaths {
                chain_state: "../test-data/genesis/integration-tests/chain_state.json".into(),
            },
            RollupProverConfig::Execute,
            NodeMode::FullNode(seq_port),
            None,
            DEFAULT_MIN_SOFT_CONFIRMATIONS_PER_COMMITMENT,
            true,
        )
        .await;
    });

    let full_node_port = full_node_port_rx.await.unwrap();
    let full_node_test_client = make_test_client(full_node_port).await;

    (
        seq_test_client,
        full_node_test_client,
        seq_task,
        full_node_task,
        Address::from_str("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266").unwrap(),
    )
}

#[tokio::test]
async fn test_soft_batch_save() -> Result<(), anyhow::Error> {
    let config = TestConfig::default();

    let (seq_port_tx, seq_port_rx) = tokio::sync::oneshot::channel();

    let seq_task = tokio::spawn(async move {
        start_rollup(
            seq_port_tx,
            GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
            BasicKernelGenesisPaths {
                chain_state: "../test-data/genesis/integration-tests/chain_state.json".into(),
            },
            RollupProverConfig::Execute,
            NodeMode::SequencerNode,
            None,
            config.seq_min_soft_confirmations,
            true,
        )
        .await;
    });

    let seq_port = seq_port_rx.await.unwrap();
    let seq_test_client = init_test_rollup(seq_port).await;

    let (full_node_port_tx, full_node_port_rx) = tokio::sync::oneshot::channel();

    let full_node_task = tokio::spawn(async move {
        start_rollup(
            full_node_port_tx,
            GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
            BasicKernelGenesisPaths {
                chain_state: "../test-data/genesis/integration-tests/chain_state.json".into(),
            },
            RollupProverConfig::Execute,
            NodeMode::FullNode(seq_port),
            None,
            DEFAULT_MIN_SOFT_CONFIRMATIONS_PER_COMMITMENT,
            true,
        )
        .await;
    });

    let full_node_port = full_node_port_rx.await.unwrap();
    let full_node_test_client = make_test_client(full_node_port).await;

    let (full_node_port_tx_2, full_node_port_rx_2) = tokio::sync::oneshot::channel();

    let full_node_task_2 = tokio::spawn(async move {
        start_rollup(
            full_node_port_tx_2,
            GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
            BasicKernelGenesisPaths {
                chain_state: "../test-data/genesis/integration-tests/chain_state.json".into(),
            },
            RollupProverConfig::Execute,
            NodeMode::FullNode(full_node_port),
            None,
            DEFAULT_MIN_SOFT_CONFIRMATIONS_PER_COMMITMENT,
            false,
        )
        .await;
    });

    let full_node_port_2 = full_node_port_rx_2.await.unwrap();
    let full_node_test_client_2 = make_test_client(full_node_port_2).await;

    let _ = execute_blocks(&seq_test_client, &full_node_test_client).await;

    sleep(Duration::from_secs(10)).await;

    let seq_block = seq_test_client
        .eth_get_block_by_number(Some(BlockNumberOrTag::Latest))
        .await;
    let full_node_block = full_node_test_client
        .eth_get_block_by_number(Some(BlockNumberOrTag::Latest))
        .await;
    let full_node_block_2 = full_node_test_client_2
        .eth_get_block_by_number(Some(BlockNumberOrTag::Latest))
        .await;

    assert_eq!(seq_block.state_root, full_node_block.state_root);
    assert_eq!(full_node_block.state_root, full_node_block_2.state_root);
    assert_eq!(seq_block.hash, full_node_block.hash);
    assert_eq!(full_node_block.hash, full_node_block_2.hash);

    seq_task.abort();
    full_node_task.abort();
    full_node_task_2.abort();

    Ok(())
}

#[tokio::test]
async fn test_full_node_send_tx() -> Result<(), anyhow::Error> {
    // citrea::initialize_logging();

    let (seq_test_client, full_node_test_client, seq_task, full_node_task, addr) =
        initialize_test(Default::default()).await;

    let tx_hash = full_node_test_client
        .send_eth(addr, None, None, None, 0u128)
        .await
        .unwrap();

    seq_test_client.send_publish_batch_request().await;

    sleep(Duration::from_millis(2000)).await;

    let sq_block = seq_test_client
        .eth_get_block_by_number(Some(BlockNumberOrTag::Latest))
        .await;

    let full_node_block = full_node_test_client
        .eth_get_block_by_number(Some(BlockNumberOrTag::Latest))
        .await;

    assert!(sq_block.transactions.contains(&tx_hash.tx_hash()));
    assert!(full_node_block.transactions.contains(&tx_hash.tx_hash()));
    assert_eq!(sq_block.state_root, full_node_block.state_root);

    seq_task.abort();
    full_node_task.abort();

    Ok(())
}

#[tokio::test]
async fn test_delayed_sync_ten_blocks() -> Result<(), anyhow::Error> {
    // citrea::initialize_logging();

    let (seq_port_tx, seq_port_rx) = tokio::sync::oneshot::channel();

    let seq_task = tokio::spawn(async {
        start_rollup(
            seq_port_tx,
            GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
            BasicKernelGenesisPaths {
                chain_state: "../test-data/genesis/integration-tests/chain_state.json".into(),
            },
            RollupProverConfig::Execute,
            NodeMode::SequencerNode,
            None,
            DEFAULT_MIN_SOFT_CONFIRMATIONS_PER_COMMITMENT,
            true,
        )
        .await;
    });

    let seq_port = seq_port_rx.await.unwrap();

    let seq_test_client = init_test_rollup(seq_port).await;
    let addr = Address::from_str("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266").unwrap();

    for _ in 0..10 {
        seq_test_client
            .send_eth(addr, None, None, None, 0u128)
            .await
            .unwrap();
        seq_test_client.send_publish_batch_request().await;
    }

    let (full_node_port_tx, full_node_port_rx) = tokio::sync::oneshot::channel();

    let full_node_task = tokio::spawn(async move {
        start_rollup(
            full_node_port_tx,
            GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
            BasicKernelGenesisPaths {
                chain_state: "../test-data/genesis/integration-tests/chain_state.json".into(),
            },
            RollupProverConfig::Execute,
            NodeMode::FullNode(seq_port),
            None,
            DEFAULT_MIN_SOFT_CONFIRMATIONS_PER_COMMITMENT,
            true,
        )
        .await;
    });

    let full_node_port = full_node_port_rx.await.unwrap();
    let full_node_test_client = make_test_client(full_node_port).await;

    sleep(Duration::from_secs(10)).await;

    let seq_block = seq_test_client
        .eth_get_block_by_number(Some(BlockNumberOrTag::Number(10)))
        .await;
    let full_node_block = full_node_test_client
        .eth_get_block_by_number(Some(BlockNumberOrTag::Number(10)))
        .await;

    assert_eq!(seq_block.state_root, full_node_block.state_root);
    assert_eq!(seq_block.hash, full_node_block.hash);

    seq_task.abort();
    full_node_task.abort();

    Ok(())
}

#[tokio::test]
async fn test_e2e_same_block_sync() -> Result<(), anyhow::Error> {
    // citrea::initialize_logging();

    let (seq_test_client, full_node_test_client, seq_task, full_node_task, _) =
        initialize_test(Default::default()).await;

    let _ = execute_blocks(&seq_test_client, &full_node_test_client).await;

    seq_task.abort();
    full_node_task.abort();

    Ok(())
}

#[tokio::test]
async fn test_close_and_reopen_full_node() -> Result<(), anyhow::Error> {
    // citrea::initialize_logging();

    // Remove temp db directories if they exist
    let _ = fs::remove_dir_all(Path::new("demo_data_test_close_and_reopen_full_node_copy"));
    let _ = fs::remove_dir_all(Path::new("demo_data_test_close_and_reopen_full_node"));

    let (seq_port_tx, seq_port_rx) = tokio::sync::oneshot::channel();

    let seq_task = tokio::spawn(async {
        start_rollup(
            seq_port_tx,
            GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
            BasicKernelGenesisPaths {
                chain_state: "../test-data/genesis/integration-tests/chain_state.json".into(),
            },
            RollupProverConfig::Execute,
            NodeMode::SequencerNode,
            None,
            DEFAULT_MIN_SOFT_CONFIRMATIONS_PER_COMMITMENT,
            true,
        )
        .await;
    });

    let seq_port = seq_port_rx.await.unwrap();

    let (full_node_port_tx, full_node_port_rx) = tokio::sync::oneshot::channel();

    // starting full node with db path
    let rollup_task = tokio::spawn(async move {
        start_rollup(
            full_node_port_tx,
            GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
            BasicKernelGenesisPaths {
                chain_state: "../test-data/genesis/integration-tests/chain_state.json".into(),
            },
            RollupProverConfig::Execute,
            NodeMode::FullNode(seq_port),
            Some("demo_data_test_close_and_reopen_full_node"),
            DEFAULT_MIN_SOFT_CONFIRMATIONS_PER_COMMITMENT,
            true,
        )
        .await;
    });

    let full_node_port = full_node_port_rx.await.unwrap();

    let seq_test_client = init_test_rollup(seq_port).await;
    let full_node_test_client = init_test_rollup(full_node_port).await;

    let addr = Address::from_str("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266").unwrap();

    // create 10 blocks
    for _ in 0..10 {
        seq_test_client
            .send_eth(addr, None, None, None, 0u128)
            .await
            .unwrap();
        seq_test_client.send_publish_batch_request().await;
    }

    // wait for full node to sync
    sleep(Duration::from_secs(5)).await;

    // check if latest blocks are the same
    let seq_last_block = seq_test_client
        .eth_get_block_by_number_with_detail(Some(BlockNumberOrTag::Latest))
        .await;

    let full_node_last_block = full_node_test_client
        .eth_get_block_by_number_with_detail(Some(BlockNumberOrTag::Latest))
        .await;

    assert_eq!(seq_last_block.number.unwrap().as_u64(), 10);
    assert_eq!(full_node_last_block.number.unwrap().as_u64(), 10);

    assert_eq!(seq_last_block.state_root, full_node_last_block.state_root);
    assert_eq!(seq_last_block.hash, full_node_last_block.hash);

    // close full node
    rollup_task.abort();

    sleep(Duration::from_secs(2)).await;

    // create 100 more blocks
    for _ in 0..100 {
        seq_test_client
            .send_eth(addr, None, None, None, 0u128)
            .await
            .unwrap();
        seq_test_client.send_publish_batch_request().await;
    }

    let da_service = MockDaService::new(MockAddress::from([0; 32]));
    da_service.publish_test_block().await.unwrap();

    // start full node again
    let (full_node_port_tx, full_node_port_rx) = tokio::sync::oneshot::channel();

    // Copy the db to a new path with the same contents because
    // the lock is not released on the db directory even though the task is aborted
    let _ = copy_dir_recursive(
        Path::new("demo_data_test_close_and_reopen_full_node"),
        Path::new("demo_data_test_close_and_reopen_full_node_copy"),
    );

    sleep(Duration::from_secs(5)).await;

    // spin up the full node again with the same data where it left of only with different path to not stuck on lock
    let rollup_task = tokio::spawn(async move {
        start_rollup(
            full_node_port_tx,
            GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
            BasicKernelGenesisPaths {
                chain_state: "../test-data/genesis/integration-tests/chain_state.json".into(),
            },
            RollupProverConfig::Execute,
            NodeMode::FullNode(seq_port),
            Some("demo_data_test_close_and_reopen_full_node_copy"),
            DEFAULT_MIN_SOFT_CONFIRMATIONS_PER_COMMITMENT,
            true,
        )
        .await;
    });

    // TODO: There should be a better way to test this?
    sleep(Duration::from_secs(10)).await;

    let full_node_port = full_node_port_rx.await.unwrap();

    let full_node_test_client = make_test_client(full_node_port).await;

    // check if the latest block state roots are same
    let seq_last_block = seq_test_client
        .eth_get_block_by_number_with_detail(Some(BlockNumberOrTag::Latest))
        .await;

    let full_node_last_block = full_node_test_client
        .eth_get_block_by_number_with_detail(Some(BlockNumberOrTag::Latest))
        .await;

    assert_eq!(seq_last_block.number.unwrap().as_u64(), 110);
    assert_eq!(full_node_last_block.number.unwrap().as_u64(), 110);

    assert_eq!(seq_last_block.state_root, full_node_last_block.state_root);
    assert_eq!(seq_last_block.hash, full_node_last_block.hash);

    fs::remove_dir_all(Path::new("demo_data_test_close_and_reopen_full_node_copy")).unwrap();
    fs::remove_dir_all(Path::new("demo_data_test_close_and_reopen_full_node")).unwrap();

    seq_task.abort();
    rollup_task.abort();

    Ok(())
}

#[tokio::test]
async fn test_get_transaction_by_hash() -> Result<(), anyhow::Error> {
    // citrea::initialize_logging();

    let (seq_port_tx, seq_port_rx) = tokio::sync::oneshot::channel();

    let seq_task = tokio::spawn(async {
        start_rollup(
            seq_port_tx,
            GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
            BasicKernelGenesisPaths {
                chain_state: "../test-data/genesis/integration-tests/chain_state.json".into(),
            },
            RollupProverConfig::Execute,
            NodeMode::SequencerNode,
            None,
            DEFAULT_MIN_SOFT_CONFIRMATIONS_PER_COMMITMENT,
            true,
        )
        .await;
    });

    let seq_port = seq_port_rx.await.unwrap();

    let (full_node_port_tx, full_node_port_rx) = tokio::sync::oneshot::channel();

    let rollup_task = tokio::spawn(async move {
        start_rollup(
            full_node_port_tx,
            GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
            BasicKernelGenesisPaths {
                chain_state: "../test-data/genesis/integration-tests/chain_state.json".into(),
            },
            RollupProverConfig::Execute,
            NodeMode::FullNode(seq_port),
            None,
            DEFAULT_MIN_SOFT_CONFIRMATIONS_PER_COMMITMENT,
            true,
        )
        .await;
    });

    let full_node_port = full_node_port_rx.await.unwrap();

    let seq_test_client = init_test_rollup(seq_port).await;
    let full_node_test_client = init_test_rollup(full_node_port).await;

    // create some txs to test the use cases
    let addr = Address::from_str("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92265").unwrap();

    let pending_tx1 = seq_test_client
        .send_eth(addr, None, None, None, 1_000_000_000u128)
        .await
        .unwrap();

    let pending_tx2 = seq_test_client
        .send_eth(addr, None, None, None, 1_000_000_000u128)
        .await
        .unwrap();
    // currently there are two txs in the pool, the full node should be able to get them
    // should get with mempool_only true
    let tx1 = full_node_test_client
        .eth_get_transaction_by_hash(pending_tx1.tx_hash(), Some(true))
        .await
        .unwrap();
    // Should get with mempool_only false/none
    let tx2 = full_node_test_client
        .eth_get_transaction_by_hash(pending_tx2.tx_hash(), None)
        .await
        .unwrap();
    assert!(tx1.block_hash.is_none());
    assert!(tx2.block_hash.is_none());
    assert_eq!(tx1.hash, pending_tx1.tx_hash());
    assert_eq!(tx2.hash, pending_tx2.tx_hash());

    // sequencer should also be able to get them
    // Should get just by checking the pool
    let tx1 = seq_test_client
        .eth_get_transaction_by_hash(pending_tx1.tx_hash(), Some(true))
        .await
        .unwrap();
    let tx2 = seq_test_client
        .eth_get_transaction_by_hash(pending_tx2.tx_hash(), None)
        .await
        .unwrap();
    assert!(tx1.block_hash.is_none());
    assert!(tx2.block_hash.is_none());
    assert_eq!(tx1.hash, pending_tx1.tx_hash());
    assert_eq!(tx2.hash, pending_tx2.tx_hash());

    seq_test_client.send_publish_batch_request().await;

    // wait for the full node to sync
    sleep(Duration::from_millis(2000)).await;

    // make sure txs are in the block
    let seq_block = seq_test_client
        .eth_get_block_by_number(Some(BlockNumberOrTag::Latest))
        .await;
    assert!(seq_block.transactions.contains(&pending_tx1.tx_hash()));
    assert!(seq_block.transactions.contains(&pending_tx2.tx_hash()));

    // same operations after the block is published, both sequencer and full node should be able to get them.
    // should not get with mempool_only true because it checks the sequencer mempool only
    let non_existent_tx = full_node_test_client
        .eth_get_transaction_by_hash(pending_tx1.tx_hash(), Some(true))
        .await;
    // this should be none because it is not in the mempool anymore
    assert!(non_existent_tx.is_none());

    let tx1 = full_node_test_client
        .eth_get_transaction_by_hash(pending_tx1.tx_hash(), Some(false))
        .await
        .unwrap();
    let tx2 = full_node_test_client
        .eth_get_transaction_by_hash(pending_tx2.tx_hash(), None)
        .await
        .unwrap();
    assert!(tx1.block_hash.is_some());
    assert!(tx2.block_hash.is_some());
    assert_eq!(tx1.hash, pending_tx1.tx_hash());
    assert_eq!(tx2.hash, pending_tx2.tx_hash());

    // should not get with mempool_only true because it checks mempool only
    let none_existent_tx = seq_test_client
        .eth_get_transaction_by_hash(pending_tx1.tx_hash(), Some(true))
        .await;
    // this should be none because it is not in the mempool anymore
    assert!(none_existent_tx.is_none());

    // In other cases should check the block and find the tx
    let tx1 = seq_test_client
        .eth_get_transaction_by_hash(pending_tx1.tx_hash(), Some(false))
        .await
        .unwrap();
    let tx2 = seq_test_client
        .eth_get_transaction_by_hash(pending_tx2.tx_hash(), None)
        .await
        .unwrap();
    assert!(tx1.block_hash.is_some());
    assert!(tx2.block_hash.is_some());
    assert_eq!(tx1.hash, pending_tx1.tx_hash());
    assert_eq!(tx2.hash, pending_tx2.tx_hash());

    // create random tx hash and make sure it returns None
    let random_tx_hash: TxHash = TxHash::random();
    assert!(seq_test_client
        .eth_get_transaction_by_hash(H256::from_slice(random_tx_hash.as_slice()), None)
        .await
        .is_none());
    assert!(full_node_test_client
        .eth_get_transaction_by_hash(H256::from_slice(random_tx_hash.as_slice()), None)
        .await
        .is_none());

    seq_task.abort();
    rollup_task.abort();
    Ok(())
}

#[tokio::test]
async fn test_soft_confirmations_on_different_blocks() -> Result<(), anyhow::Error> {
    // citrea::initialize_logging();

    let da_service = MockDaService::new(MockAddress::default());

    let (seq_test_client, full_node_test_client, seq_task, full_node_task, _) =
        initialize_test(Default::default()).await;

    // first publish a few blocks fast make it land in the same da block
    for _ in 1..=6 {
        seq_test_client.send_publish_batch_request().await;
    }

    sleep(Duration::from_secs(2)).await;

    let mut last_da_slot_height = 0;
    let mut last_da_slot_hash = <MockDaSpec as DaSpec>::SlotHash::from([0u8; 32]);

    // now retrieve soft confirmations from the sequencer and full node and check if they are the same
    for i in 1..=6 {
        let seq_soft_conf = seq_test_client
            .ledger_get_soft_batch_by_number::<MockDaSpec>(i)
            .await
            .unwrap();
        let full_node_soft_conf = full_node_test_client
            .ledger_get_soft_batch_by_number::<MockDaSpec>(i)
            .await
            .unwrap();

        if i != 1 {
            assert_eq!(last_da_slot_height, seq_soft_conf.da_slot_height);
            assert_eq!(last_da_slot_hash, MockHash(seq_soft_conf.da_slot_hash));
        }

        assert_eq!(
            seq_soft_conf.da_slot_height,
            full_node_soft_conf.da_slot_height
        );

        assert_eq!(seq_soft_conf.da_slot_hash, full_node_soft_conf.da_slot_hash);

        last_da_slot_height = seq_soft_conf.da_slot_height;
        last_da_slot_hash = MockHash(seq_soft_conf.da_slot_hash);
    }

    // publish new da block
    da_service.publish_test_block().await.unwrap();

    for _ in 1..=6 {
        seq_test_client.spam_publish_batch_request().await.unwrap();
    }

    sleep(Duration::from_secs(2)).await;

    for i in 7..=12 {
        let seq_soft_conf = seq_test_client
            .ledger_get_soft_batch_by_number::<MockDaSpec>(i)
            .await
            .unwrap();
        let full_node_soft_conf = full_node_test_client
            .ledger_get_soft_batch_by_number::<MockDaSpec>(i)
            .await
            .unwrap();

        if i != 7 {
            assert_eq!(last_da_slot_height, seq_soft_conf.da_slot_height);
            assert_eq!(last_da_slot_hash, MockHash(seq_soft_conf.da_slot_hash));
        } else {
            assert_ne!(last_da_slot_height, seq_soft_conf.da_slot_height);
            assert_ne!(last_da_slot_hash, MockHash(seq_soft_conf.da_slot_hash));
        }

        assert_eq!(
            seq_soft_conf.da_slot_height,
            full_node_soft_conf.da_slot_height
        );

        assert_eq!(seq_soft_conf.da_slot_hash, full_node_soft_conf.da_slot_hash);

        last_da_slot_height = seq_soft_conf.da_slot_height;
        last_da_slot_hash = MockHash(seq_soft_conf.da_slot_hash);
    }

    seq_task.abort();
    full_node_task.abort();

    Ok(())
}

#[tokio::test]
async fn test_reopen_sequencer() -> Result<(), anyhow::Error> {
    // open, close without publishing blokcs
    // then reopen, publish some blocks without error
    // Remove temp db directories if they exist
    let _ = fs::remove_dir_all(Path::new("demo_data_test_reopen_sequencer_copy"));
    let _ = fs::remove_dir_all(Path::new("demo_data_test_reopen_sequencer"));

    let (seq_port_tx, seq_port_rx) = tokio::sync::oneshot::channel();

    let seq_task = tokio::spawn(async {
        start_rollup(
            seq_port_tx,
            GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
            BasicKernelGenesisPaths {
                chain_state: "../test-data/genesis/integration-tests/chain_state.json".into(),
            },
            RollupProverConfig::Execute,
            NodeMode::SequencerNode,
            Some("demo_data_test_reopen_sequencer"),
            DEFAULT_MIN_SOFT_CONFIRMATIONS_PER_COMMITMENT,
            true,
        )
        .await;
    });

    let seq_port = seq_port_rx.await.unwrap();

    let seq_test_client = init_test_rollup(seq_port).await;

    let block = seq_test_client
        .eth_get_block_by_number(Some(BlockNumberOrTag::Latest))
        .await;
    assert_eq!(block.number.unwrap().as_u64(), 0);

    // close sequencer
    seq_task.abort();

    sleep(Duration::from_secs(1)).await;

    let (seq_port_tx, seq_port_rx) = tokio::sync::oneshot::channel();

    // Copy the db to a new path with the same contents because
    // the lock is not released on the db directory even though the task is aborted
    let _ = copy_dir_recursive(
        Path::new("demo_data_test_reopen_sequencer"),
        Path::new("demo_data_test_reopen_sequencer_copy"),
    );

    let da_service = MockDaService::new(MockAddress::from([0; 32]));
    da_service.publish_test_block().await.unwrap();

    sleep(Duration::from_secs(1)).await;

    let seq_task = tokio::spawn(async {
        start_rollup(
            seq_port_tx,
            GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
            BasicKernelGenesisPaths {
                chain_state: "../test-data/genesis/integration-tests/chain_state.json".into(),
            },
            RollupProverConfig::Execute,
            NodeMode::SequencerNode,
            Some("demo_data_test_reopen_sequencer_copy"),
            DEFAULT_MIN_SOFT_CONFIRMATIONS_PER_COMMITMENT,
            true,
        )
        .await;
    });

    let seq_port = seq_port_rx.await.unwrap();

    let seq_test_client = make_test_client(seq_port).await;

    let seq_last_block = seq_test_client
        .eth_get_block_by_number(Some(BlockNumberOrTag::Latest))
        .await;

    // make sure the state roots are the same
    assert_eq!(seq_last_block.state_root, block.state_root);
    assert_eq!(
        seq_last_block.number.unwrap().as_u64(),
        block.number.unwrap().as_u64()
    );

    seq_test_client.send_publish_batch_request().await;
    seq_test_client.send_publish_batch_request().await;

    assert_eq!(
        seq_test_client
            .eth_get_block_by_number(Some(BlockNumberOrTag::Latest))
            .await
            .number
            .unwrap()
            .as_u64(),
        2
    );

    fs::remove_dir_all(Path::new("demo_data_test_reopen_sequencer_copy")).unwrap();
    fs::remove_dir_all(Path::new("demo_data_test_reopen_sequencer")).unwrap();

    seq_task.abort();

    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    if !dst.exists() {
        fs::create_dir(dst)?;
    }

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let entry_path = entry.path();
        let target_path = dst.join(entry.file_name());

        if entry_path.is_dir() {
            copy_dir_recursive(&entry_path, &target_path)?;
        } else {
            fs::copy(&entry_path, &target_path)?;
        }
    }
    Ok(())
}

async fn execute_blocks(
    sequencer_client: &TestClient,
    full_node_client: &TestClient,
) -> Result<(), Box<dyn std::error::Error>> {
    let (contract_address, contract) = {
        let contract = SimpleStorageContract::default();
        let deploy_contract_req = sequencer_client
            .deploy_contract(contract.byte_code(), None)
            .await?;
        sequencer_client.send_publish_batch_request().await;

        let contract_address = deploy_contract_req
            .await?
            .unwrap()
            .contract_address
            .unwrap();

        (contract_address, contract)
    };

    {
        let set_value_req = sequencer_client
            .contract_transaction(contract_address, contract.set_call_data(42), None)
            .await;
        sequencer_client.send_publish_batch_request().await;
        set_value_req.await.unwrap().unwrap();
    }

    sequencer_client.send_publish_batch_request().await;

    {
        for temp in 0..10 {
            let _set_value_req = sequencer_client
                .contract_transaction(contract_address, contract.set_call_data(78 + temp), None)
                .await;
        }
        sequencer_client.send_publish_batch_request().await;
    }

    {
        for _ in 0..200 {
            sequencer_client.spam_publish_batch_request().await.unwrap();
        }

        sleep(Duration::from_secs(1)).await;
    }

    let da_service = MockDaService::new(MockAddress::from([0; 32]));
    da_service.publish_test_block().await.unwrap();

    {
        let addr = Address::from_str("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266").unwrap();

        for _ in 0..300 {
            sequencer_client
                .send_eth(addr, None, None, None, 0u128)
                .await
                .unwrap();
            sequencer_client.spam_publish_batch_request().await.unwrap();
        }
    }

    sleep(Duration::from_millis(5000)).await;

    let seq_last_block = sequencer_client
        .eth_get_block_by_number_with_detail(Some(BlockNumberOrTag::Latest))
        .await;

    let full_node_last_block = full_node_client
        .eth_get_block_by_number_with_detail(Some(BlockNumberOrTag::Latest))
        .await;

    assert_eq!(seq_last_block.number.unwrap().as_u64(), 504);
    assert_eq!(full_node_last_block.number.unwrap().as_u64(), 504);

    assert_eq!(seq_last_block.state_root, full_node_last_block.state_root);
    assert_eq!(seq_last_block.hash, full_node_last_block.hash);

    Ok(())
}

#[tokio::test]
async fn test_soft_confirmations_status_one_l1() -> Result<(), anyhow::Error> {
    // citrea::initialize_logging();

    let da_service = MockDaService::new(MockAddress::default());

    let (seq_test_client, full_node_test_client, seq_task, full_node_task, _) =
        initialize_test(TestConfig {
            seq_min_soft_confirmations: 3,
        })
        .await;

    // first publish a few blocks fast make it land in the same da block
    for _ in 1..=6 {
        seq_test_client.send_publish_batch_request().await;
    }

    // TODO check status=trusted

    sleep(Duration::from_secs(2)).await;

    // publish new da block
    da_service.publish_test_block().await.unwrap();
    seq_test_client.send_publish_batch_request().await; // TODO https://github.com/chainwayxyz/citrea/issues/214
    seq_test_client.send_publish_batch_request().await; // TODO https://github.com/chainwayxyz/citrea/issues/214

    sleep(Duration::from_secs(2)).await;

    // now retrieve confirmation status from the sequencer and full node and check if they are the same
    for i in 1..=6 {
        let status_node = full_node_test_client
            .ledger_get_soft_confirmation_status(i)
            .await
            .unwrap();

        assert_eq!(SoftConfirmationStatus::Finalized, status_node.unwrap());
    }

    seq_task.abort();
    full_node_task.abort();

    Ok(())
}

#[tokio::test]
async fn test_soft_confirmations_status_two_l1() -> Result<(), anyhow::Error> {
    // citrea::initialize_logging();

    let da_service = MockDaService::new(MockAddress::default());

    let (seq_test_client, full_node_test_client, seq_task, full_node_task, _) =
        initialize_test(TestConfig {
            seq_min_soft_confirmations: 3,
        })
        .await;

    // first publish a few blocks fast make it land in the same da block
    for _ in 1..=2 {
        seq_test_client.send_publish_batch_request().await;
    }

    sleep(Duration::from_secs(2)).await;

    // publish new da block
    da_service.publish_test_block().await.unwrap();

    for _ in 2..=6 {
        seq_test_client.send_publish_batch_request().await;
    }

    // now retrieve confirmation status from the sequencer and full node and check if they are the same
    for i in 1..=2 {
        let status_node = full_node_test_client
            .ledger_get_soft_confirmation_status(i)
            .await
            .unwrap();

        assert_eq!(SoftConfirmationStatus::Trusted, status_node.unwrap());
    }

    // publish new da block
    da_service.publish_test_block().await.unwrap();
    seq_test_client.send_publish_batch_request().await; // TODO https://github.com/chainwayxyz/citrea/issues/214
    seq_test_client.send_publish_batch_request().await; // TODO https://github.com/chainwayxyz/citrea/issues/214

    sleep(Duration::from_secs(2)).await;

    // Check that these L2 blocks are bounded on different L1 block
    let mut batch_infos = vec![];
    for i in 1..=6 {
        let full_node_soft_conf = full_node_test_client
            .ledger_get_soft_batch_by_number::<MockDaSpec>(i)
            .await
            .unwrap();
        batch_infos.push(full_node_soft_conf);
    }
    assert_eq!(batch_infos[0].da_slot_height, batch_infos[1].da_slot_height);
    assert!(batch_infos[2..]
        .iter()
        .all(|x| x.da_slot_height == batch_infos[2].da_slot_height));
    assert_ne!(batch_infos[0].da_slot_height, batch_infos[5].da_slot_height);

    // now retrieve confirmation status from the sequencer and full node and check if they are the same
    for i in 1..=6 {
        let status_node = full_node_test_client
            .ledger_get_soft_confirmation_status(i)
            .await
            .unwrap();

        assert_eq!(SoftConfirmationStatus::Finalized, status_node.unwrap());
    }

    let status_node = full_node_test_client
        .ledger_get_soft_confirmation_status(410)
        .await;

    assert!(format!("{:?}", status_node.err())
        .contains("Soft confirmation at height 410 not processed yet."));

    seq_task.abort();
    full_node_task.abort();

    Ok(())
}

#[tokio::test]
async fn test_prover_sync_with_commitments() -> Result<(), anyhow::Error> {
    // citrea::initialize_logging();
    let da_service = MockDaService::new(MockAddress::default());

    let (seq_port_tx, seq_port_rx) = tokio::sync::oneshot::channel();

    let seq_task = tokio::spawn(async move {
        start_rollup(
            seq_port_tx,
            GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
            BasicKernelGenesisPaths {
                chain_state: "../test-data/genesis/integration-tests/chain_state.json".into(),
            },
            RollupProverConfig::Execute,
            NodeMode::SequencerNode,
            None,
            4,
            true,
        )
        .await;
    });

    let seq_port = seq_port_rx.await.unwrap();
    let seq_test_client = make_test_client(seq_port).await;

    let (prover_node_port_tx, prover_node_port_rx) = tokio::sync::oneshot::channel();

    let prover_node_task = tokio::spawn(async move {
        start_rollup(
            prover_node_port_tx,
            GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
            BasicKernelGenesisPaths {
                chain_state: "../test-data/genesis/integration-tests/chain_state.json".into(),
            },
            RollupProverConfig::Execute,
            NodeMode::Prover(seq_port),
            None,
            4,
            true,
        )
        .await;
    });

    let prover_node_port = prover_node_port_rx.await.unwrap();
    let prover_node_test_client = make_test_client(prover_node_port).await;

    // publish 3 soft confirmations, no commitment should be sent
    for _ in 0..3 {
        seq_test_client.send_publish_batch_request().await;
    }

    sleep(Duration::from_secs(2)).await;

    // prover should not have any blocks saved
    assert_eq!(prover_node_test_client.eth_block_number().await, 0);

    da_service.publish_test_block().await.unwrap();

    seq_test_client.send_publish_batch_request().await;

    // sequencer commitment should be sent
    da_service.publish_test_block().await.unwrap();
    // start l1 height = 1, end = 2
    seq_test_client.send_publish_batch_request().await;

    // wait for prover to sync
    sleep(Duration::from_secs(5)).await;

    // prover should have synced all 4 l2 blocks
    assert_eq!(prover_node_test_client.eth_block_number().await, 4);

    seq_test_client.send_publish_batch_request().await;

    sleep(Duration::from_secs(3)).await;

    // Still should have 4 blokcs there are no commitments yet
    assert_eq!(prover_node_test_client.eth_block_number().await, 4);

    seq_test_client.send_publish_batch_request().await;
    seq_test_client.send_publish_batch_request().await;
    sleep(Duration::from_secs(3)).await;
    // Still should have 4 blokcs there are no commitments yet
    assert_eq!(prover_node_test_client.eth_block_number().await, 4);
    da_service.publish_test_block().await.unwrap();

    // Commitment is sent right before the 9th block is published
    seq_test_client.send_publish_batch_request().await;

    // Wait for prover to sync
    sleep(Duration::from_secs(5)).await;
    // Should now have 8 blocks = 2 commitments of blocks 1-4 and 5-8
    assert_eq!(prover_node_test_client.eth_block_number().await, 8);

    // TODO: Also test with multiple commitments in single Mock DA Block
    seq_task.abort();
    prover_node_task.abort();
    Ok(())
}

#[tokio::test]
async fn test_reopen_prover() -> Result<(), anyhow::Error> {
    let _ = fs::remove_dir_all(Path::new("demo_data_test_reopen_prover_copy2"));
    let _ = fs::remove_dir_all(Path::new("demo_data_test_reopen_prover_copy"));
    let _ = fs::remove_dir_all(Path::new("demo_data_test_reopen_prover"));

    let da_service = MockDaService::new(MockAddress::default());

    let (seq_port_tx, seq_port_rx) = tokio::sync::oneshot::channel();

    let seq_task = tokio::spawn(async move {
        start_rollup(
            seq_port_tx,
            GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
            BasicKernelGenesisPaths {
                chain_state: "../test-data/genesis/integration-tests/chain_state.json".into(),
            },
            RollupProverConfig::Execute,
            NodeMode::SequencerNode,
            None,
            4,
            true,
        )
        .await;
    });

    let seq_port = seq_port_rx.await.unwrap();
    let seq_test_client = make_test_client(seq_port).await;

    let (prover_node_port_tx, prover_node_port_rx) = tokio::sync::oneshot::channel();

    let prover_node_task = tokio::spawn(async move {
        start_rollup(
            prover_node_port_tx,
            GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
            BasicKernelGenesisPaths {
                chain_state: "../test-data/genesis/integration-tests/chain_state.json".into(),
            },
            RollupProverConfig::Execute,
            NodeMode::Prover(seq_port),
            Some("demo_data_test_reopen_prover"),
            4,
            true,
        )
        .await;
    });

    let prover_node_port = prover_node_port_rx.await.unwrap();
    let prover_node_test_client = make_test_client(prover_node_port).await;

    // publish 3 soft confirmations, no commitment should be sent
    for _ in 0..3 {
        seq_test_client.send_publish_batch_request().await;
    }

    sleep(Duration::from_secs(2)).await;

    // prover should not have any blocks saved
    assert_eq!(prover_node_test_client.eth_block_number().await, 0);

    da_service.publish_test_block().await.unwrap();

    seq_test_client.send_publish_batch_request().await;

    // sequencer commitment should be sent
    da_service.publish_test_block().await.unwrap();
    // start l1 height = 1, end = 2
    seq_test_client.send_publish_batch_request().await;

    // wait for prover to sync
    sleep(Duration::from_secs(5)).await;

    // prover should have synced all 4 l2 blocks
    assert_eq!(prover_node_test_client.eth_block_number().await, 4);

    prover_node_task.abort();
    let _ = copy_dir_recursive(
        Path::new("demo_data_test_reopen_prover"),
        Path::new("demo_data_test_reopen_prover_copy"),
    );

    // Reopen prover with the new path
    let (prover_node_port_tx, prover_node_port_rx) = tokio::sync::oneshot::channel();

    let prover_node_task = tokio::spawn(async move {
        start_rollup(
            prover_node_port_tx,
            GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
            BasicKernelGenesisPaths {
                chain_state: "../test-data/genesis/integration-tests/chain_state.json".into(),
            },
            RollupProverConfig::Execute,
            NodeMode::Prover(seq_port),
            Some("demo_data_test_reopen_prover_copy"),
            4,
            true,
        )
        .await;
    });

    let prover_node_port = prover_node_port_rx.await.unwrap();
    let prover_node_test_client = make_test_client(prover_node_port).await;

    sleep(Duration::from_secs(2)).await;

    seq_test_client.send_publish_batch_request().await;

    sleep(Duration::from_secs(3)).await;

    // Still should have 4 blokcs there are no commitments yet
    assert_eq!(prover_node_test_client.eth_block_number().await, 4);

    prover_node_task.abort();

    sleep(Duration::from_secs(2)).await;

    seq_test_client.send_publish_batch_request().await;
    seq_test_client.send_publish_batch_request().await;

    let _ = copy_dir_recursive(
        Path::new("demo_data_test_reopen_prover_copy"),
        Path::new("demo_data_test_reopen_prover_copy2"),
    );

    // Reopen prover with the new path
    let (prover_node_port_tx, prover_node_port_rx) = tokio::sync::oneshot::channel();

    let prover_node_task = tokio::spawn(async move {
        start_rollup(
            prover_node_port_tx,
            GenesisPaths::from_dir("../test-data/genesis/integration-tests"),
            BasicKernelGenesisPaths {
                chain_state: "../test-data/genesis/integration-tests/chain_state.json".into(),
            },
            RollupProverConfig::Execute,
            NodeMode::Prover(seq_port),
            Some("demo_data_test_reopen_prover_copy2"),
            4,
            true,
        )
        .await;
    });

    let prover_node_port = prover_node_port_rx.await.unwrap();
    let prover_node_test_client = make_test_client(prover_node_port).await;

    sleep(Duration::from_secs(3)).await;
    // Still should have 4 blokcs there are no commitments yet
    assert_eq!(prover_node_test_client.eth_block_number().await, 4);
    da_service.publish_test_block().await.unwrap();

    // Commitment is sent right before the 9th block is published
    seq_test_client.send_publish_batch_request().await;

    // Wait for prover to sync
    sleep(Duration::from_secs(5)).await;
    // Should now have 8 blocks = 2 commitments of blocks 1-4 and 5-8
    assert_eq!(prover_node_test_client.eth_block_number().await, 8);

    // TODO: Also test with multiple commitments in single Mock DA Block
    seq_task.abort();
    prover_node_task.abort();

    let _ = fs::remove_dir_all(Path::new("demo_data_test_reopen_prover_copy2"));
    let _ = fs::remove_dir_all(Path::new("demo_data_test_reopen_prover_copy"));
    let _ = fs::remove_dir_all(Path::new("demo_data_test_reopen_prover"));
    Ok(())
}

#[tokio::test]
async fn test_system_transactons() -> Result<(), anyhow::Error> {
    // citrea::initialize_logging();
    let l1_blockhash_contract = L1BlockHashList::default();

    let system_contract_address =
        Address::from_str("0x3100000000000000000000000000000000000001").unwrap();
    let system_signer_address =
        Address::from_str("0xdeaddeaddeaddeaddeaddeaddeaddeaddeaddead").unwrap();

    let da_service = MockDaService::new(MockAddress::default());

    // start rollup on da block 3
    for _ in 0..3 {
        da_service.publish_test_block().await.unwrap();
    }

    let (seq_test_client, full_node_test_client, seq_task, full_node_task, _) =
        initialize_test(Default::default()).await;

    // publish some blocks with system transactions
    for _ in 0..10 {
        for _ in 0..5 {
            seq_test_client.spam_publish_batch_request().await.unwrap();
        }

        da_service.publish_test_block().await.unwrap();
    }

    seq_test_client.send_publish_batch_request().await;

    sleep(Duration::from_secs(5)).await;

    // check block 1-6-11-16-21-26-31-36-41-46-51 has system transactions
    for i in 0..=10 {
        let block_num = 1 + i * 5;

        let block = full_node_test_client
            .eth_get_block_by_number_with_detail(Some(BlockNumberOrTag::Number(block_num)))
            .await;

        if block_num == 1 {
            assert_eq!(block.transactions.len(), 2);

            let init_tx = &block.transactions[0];
            let set_tx = &block.transactions[1];

            assert_eq!(init_tx.from, system_signer_address);
            assert_eq!(init_tx.to.unwrap(), system_contract_address);
            assert_eq!(
                init_tx.input[..],
                *hex::decode(
                    "1f5783330000000000000000000000000000000000000000000000000000000000000003"
                )
                .unwrap()
                .as_slice()
            );

            assert_eq!(set_tx.from, system_signer_address);
            assert_eq!(set_tx.to.unwrap(), system_contract_address);
            assert_eq!(
                set_tx.input[0..4],
                *hex::decode("0e27bc11").unwrap().as_slice()
            );
        } else {
            assert_eq!(block.transactions.len(), 1);

            let tx = &block.transactions[0];

            assert_eq!(tx.from, system_signer_address);
            assert_eq!(tx.to.unwrap(), system_contract_address);
            assert_eq!(tx.input[0..4], *hex::decode("0e27bc11").unwrap().as_slice());
        }
    }

    // and other blocks don't have
    for i in 0..=51 {
        if i % 5 == 1 {
            continue;
        }

        let block = full_node_test_client
            .eth_get_block_by_number_with_detail(Some(BlockNumberOrTag::Number(i)))
            .await;

        assert_eq!(block.transactions.len(), 0);
    }

    // now check hashes
    for i in 3..=13 {
        let da_block = da_service.get_block_at(i).await.unwrap();

        let hash_on_chain: String = full_node_test_client
            .contract_call(
                system_contract_address,
                l1_blockhash_contract.get_block_hash(i),
                None,
            )
            .await
            .unwrap();

        assert_eq!(
            &da_block.header.hash.0,
            hex::decode(hash_on_chain.clone().split_off(2))
                .unwrap()
                .as_slice()
        );

        // check block response as well
        let block = full_node_test_client
            .eth_get_block_by_number_with_detail(Some(BlockNumberOrTag::Number((i - 3) * 5 + 1)))
            .await;

        assert_eq!(block.other.get("l1Hash"), Some(&hash_on_chain.into()));
    }

    let seq_last_block = seq_test_client
        .eth_get_block_by_number(Some(BlockNumberOrTag::Latest))
        .await;
    let node_last_block = full_node_test_client
        .eth_get_block_by_number(Some(BlockNumberOrTag::Latest))
        .await;

    assert_eq!(seq_last_block, node_last_block);

    seq_task.abort();
    full_node_task.abort();

    Ok(())
}
