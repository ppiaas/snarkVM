#!/usr/bin/env bash

cargo test \
--release \
--package snarkvm-dpc \
--lib \
--features snarkvm-algorithms/cuda,snarkvm-algorithms/print-trace \
-- posw::posw::tests::test_posw_marlin \
--exact \
--nocapture

echo ""
echo ""
echo ""
wait_secs=3
echo "wait for "$wait_secs"s"
sleep ${wait_secs}s

cargo test \
--release \
--package snarkvm-dpc \
--lib \
--features snarkvm-algorithms/cuda,snarkvm-algorithms/print-trace \
-- transaction::transaction::tests::test_public_coinbase_record \
--exact \
--nocapture