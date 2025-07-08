export RUST_LOG="${RUST_LOG:-warn,r2kad=debug,kira=debug,path_id,native_fwd_table,linux,in_memory_fwd_table,derive_fwd_table_entries,explicit_path_management,precompute_paths_and_path_ids,failure_handling,vicinity_discovery}"
export OTEL_EXPORTER_OTLP_ENDPOINT="${OTEL_EXPORTER_OTLP_ENDPOINT:-http://$(ip -4 addr show docker0 | grep -oP '(?<=inet\s)\d+(\.\d+){3}'):4317}"


echo "Cleaning logs of previous runs..."
make clean-logs

echo "Starting jaeger..."
make jaeger || exit 255
python tests/nesttest.py tests/minimal.gml --otel
