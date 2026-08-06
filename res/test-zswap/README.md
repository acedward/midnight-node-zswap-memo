# To regenerate `zswap_undeployed.mn`

The fixture is a batch of proven transactions built against the undeployed genesis block, so it
has to be rebuilt whenever the transaction serialization changes.

```bash
$ ./target/release/midnight-node-toolkit generate-txs \
    --src-file res/genesis/genesis_block_undeployed.mn \
    --dust-warp \
    --dest-file res/test-zswap/zswap_undeployed.mn \
    batches -n 1 -b 1 \
    --rng-seed 0000000000000000000000000000000000000000000000000000000000000037
```

The `--rng-seed` is the one the Earthfile uses, so regenerating by hand and regenerating via
`earthly -P +rebuild-genesis-state --NETWORK=undeployed --GENERATE_TEST_TXS=true` agree. The
Earthly target is the better route when it is available: it rebuilds the genesis pair and every
other test transaction in the same pass, keeping them consistent with each other.
