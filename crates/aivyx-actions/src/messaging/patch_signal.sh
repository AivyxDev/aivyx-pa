#!/bin/bash
# Wrap jsonrpc_unix in cfg(unix) and provide fallback
sed -i 's/async fn jsonrpc_unix/#[cfg(unix)]\nasync fn jsonrpc_unix/' signal.rs
cat << 'ENDBLOCK' >> signal.rs

#[cfg(not(unix))]
async fn jsonrpc_unix(_socket_path: &str, _payload: &serde_json::Value) -> Result<String> {
    Err(AivyxError::Channel("Signal Unix Sockets are not supported on Windows. TCP bindings not yet established.".into()))
}
ENDBLOCK
