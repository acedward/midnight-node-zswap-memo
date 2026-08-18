#toolkit #runtime
# Recognize runtime spec version 2.2.0 in block fetcher

The toolkit block fetcher now maps `spec_version` `2_002_000` to 2.2.0. That runtime adds
pallet-midnight storage, genesis and error variants but leaves the extrinsic envelope
(`send_mn_transaction` / `send_mn_system_transaction` / `timestamp.set`) unchanged, so its blocks
decode with the 2.1.0 subxt metadata.
