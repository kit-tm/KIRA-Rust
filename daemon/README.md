# R²/Kad Routing Daemon Implementation

Rust crate representing the binary implementation of the R²/Kad routing daemon.
For overall instructions please see the workspace
repository [README](https://git.scc.kit.edu/ubesd/r2kad/-/blob/main/).

## Test Framework

The test framework is custom-built to be able to run the test cases as integration tests instead of manual ones.
The integration tests themselves are located in the folder [`tests`](./tests).
The test framework and its structures are located in the module [`common`](./tests/common).

The integrations tests yield trace logs in files located in the log directory `log/integration_test/{test_name}`.
The network of the integration tests is constructed with Threads and in memory message channels (Cables).
Every thread runs a Node and they communicate through the cables, which can be closed at runtime.
Therefore, the files in the log directory are structured by thread and are identified by the node id of the associated
node.
