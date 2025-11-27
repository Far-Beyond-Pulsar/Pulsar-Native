# Multiuser Protocol Specification

## Overview
This document specifies the WebSocket-based protocol for real-time collaboration and file synchronization in Pulsar.

## Connection Flow

### 1. Session Creation (Host)
```
Client -> Server: Connect to WebSocket
Client -> Server: ClientMessage::Join { session_id, peer_id, join_token }
Server -> Client: ServerMessage::Joined { session_id, peer_id, participants }
```

### 2. Session Join (Guest)
```
Client -> Server: Connect to WebSocket
Client -> Server: ClientMessage::Join { session_id, peer_id, join_token }
Server -> Client: ServerMessage::Joined { session_id, peer_id, participants }
Server -> All Others: ServerMessage::PeerJoined { session_id, peer_id }
```

## File Synchronization Protocol

### Phase 1: Manifest Exchange
**Guest requests project manifest from Host:**
```
Guest -> Server: ClientMessage::RequestFileManifest { session_id, peer_id }
Server -> Host: ServerMessage::RequestFileManifest { session_id, from_peer_id }
Host -> Server: ClientMessage::FileManifest { session_id, peer_id, manifest_json }
Server -> Guest: ServerMessage::FileManifest { session_id, from_peer_id, manifest_json }
```

**manifest_json structure:**
```json
{
  "files": [
    {
      "path": "relative/path/to/file.rs",
      "hash": "sha256_hash",
      "size": 1234,
      "modified": 1234567890
    }
  ]
}
```

### Phase 2: File Transfer
**Guest calculates diff and requests missing/changed files:**
```
Guest -> Server: ClientMessage::RequestFiles { session_id, peer_id, file_paths: Vec<String> }
Server -> Host: ServerMessage::RequestFiles { session_id, from_peer_id, file_paths }
Host -> Server: ClientMessage::FilesChunk { session_id, peer_id, files_json, chunk_index, total_chunks }
Server -> Guest: ServerMessage::FilesChunk { session_id, from_peer_id, files_json, chunk_index, total_chunks }
```

**files_json structure (each chunk):**
```json
[
  ["path/to/file1.rs", [/* file bytes as array */]],
  ["path/to/file2.rs", [/* file bytes as array */]]
]
```

### Phase 3: UI Presentation
**Guest presents diff to user:**
1. Calculate SyncDiff (files to add, update, delete)
2. Populate diff UI with before/after content
3. User reviews changes in side-by-side editor
4. User accepts/rejects changes
5. If accepted, write files to disk

## Chat Protocol

### Send Chat Message
```
Client -> Server: ClientMessage::ChatMessage { session_id, peer_id, message }
Server -> All: ServerMessage::ChatMessage { session_id, peer_id, message, timestamp }
```

## Error Handling

### Server Errors
```
Server -> Client: ServerMessage::Error { message }
```

**Common error scenarios:**
- Invalid session_id
- Invalid join_token
- Session full
- Peer disconnected during transfer
- File read/write errors

## Message Format

### Wire Format
All messages are sent as WebSocket text frames containing JSON:
```json
{
  "type": "message_type_in_snake_case",
  "field1": "value1",
  "field2": "value2"
}
```

### Type Tags
- ClientMessage types: `join`, `leave`, `chat_message`, `request_file_manifest`, `file_manifest`, `request_files`, `files_chunk`
- ServerMessage types: `joined`, `peer_joined`, `peer_left`, `chat_message`, `request_file_manifest`, `file_manifest`, `request_files`, `files_chunk`, `error`

## Implementation Requirements

### Client Requirements
1. ✅ Connect to WebSocket server
2. ✅ Send Join message with credentials
3. ✅ Handle Joined response
4. ✅ Listen for PeerJoined/PeerLeft events
5. ✅ Implement file manifest generation (Host)
6. ✅ Implement diff calculation (Guest)
7. ✅ Implement file chunking for large transfers
8. ✅ Present diff UI to user
9. ⚠️ Handle network errors and retries
10. ⚠️ Implement timeout handling

### Server Requirements
1. ⚠️ Accept WebSocket connections
2. ⚠️ Validate Join messages
3. ⚠️ Maintain session state
4. ⚠️ Relay messages between peers
5. ⚠️ Handle peer disconnection
6. ⚠️ Send error messages for invalid requests
7. ⚠️ Implement session cleanup
8. ⚠️ Rate limiting and abuse prevention

## Future Enhancements
- Binary frame support for faster file transfer
- Compression for large files
- Resume capability for interrupted transfers
- P2P direct connection after initial handshake
- Real-time cursor/selection synchronization
- Conflict resolution for concurrent edits
