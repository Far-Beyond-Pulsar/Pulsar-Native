use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use socket2::{Domain, Protocol, Socket, Type};

use serde::Serialize;

use crate::cell::CellStorage;
use crate::cell_type::RegisteredCellType;
use crate::component::ComponentId;
use crate::gpu::{CellId, GpuBufferDispatch, SceneGpuStore};
use crate::page::Pod;
use crate::token::TypeToken;

// ── Telemetry data types ──────────────────────────────────────────────────

/// Complete snapshot of SceneDB state, collected from the main thread
/// and served to HTTP clients by the background telemetry server.
#[derive(Serialize)]
pub struct TelemetrySnapshot {
    /// Per-cell storage data (from CellStorage).
    pub cells: Vec<CellSnapshot>,
    /// GPU store metadata (from SceneGpuStore).
    pub gpu: GpuSnapshot,
    /// Registered component types → column layouts.
    pub schema: Vec<TypeSchema>,
    /// Row pool + slot pool status.
    pub pools: PoolSnapshot,
}

#[derive(Serialize)]
pub struct CellSnapshot {
    pub id: u32,
    pub rows_in_use: u32,
    pub capacity: u32,
    pub user_column_count: usize,
    /// Token→column index map for Pod columns.
    pub pod_columns: Vec<(u32, usize)>,
    /// Generic column type IDs → column index.
    pub generic_columns: Vec<(String, usize)>,
    /// Pod column data: component_id → rows of raw hex bytes.
    pub pod_data: Vec<ColumnData>,
    /// Slot column (one u32 per row).
    pub slot_column: Vec<u32>,
    /// Liveness mask: rows 0..rows_in_use, packed bits.
    pub liveness_bits: Vec<u64>,
    /// Registered cell type name, if available.
    pub cell_type_name: String,
}

#[derive(Serialize)]
pub struct ColumnData {
    pub component_id: u32,
    pub element_size: usize,
    /// Hex-encoded raw bytes, row-major: [row0_bytes, row1_bytes, ...]
    pub rows_hex: Vec<String>,
}

#[derive(Serialize)]
pub struct GpuSnapshot {
    pub gen_writes: u64,
    pub buffers: Vec<GpuBufferSnapshot>,
    /// Per-cell GPU state (dirty column counts, pending retires).
    pub cell_gpu_states: Vec<CellGpuSnapshot>,
}

#[derive(Serialize)]
pub struct GpuBufferSnapshot {
    pub component_id: u32,
    pub element_size: usize,
    pub capacity: u32,
}

#[derive(Serialize)]
pub struct CellGpuSnapshot {
    pub id: u32,
    pub class: usize,
    pub row_base: u32,
    pub slot_base: u32,
    pub slot_capacity: u32,
    pub dirty_column_count: usize,
    pub pending_retire_count: usize,
    pub alive: bool,
}

#[derive(Serialize)]
pub struct TypeSchema {
    pub component_id: u32,
    pub type_name: String,
    pub size: usize,
    pub align: usize,
}

#[derive(Serialize)]
pub struct PoolSnapshot {
    pub row: Vec<PoolInfo>,
    pub slot: Vec<PoolInfo>,
}

#[derive(Serialize)]
pub struct PoolInfo {
    pub region_size: u32,
    pub total: u32,
    pub free: u32,
}

// ── Snapshot collection ───────────────────────────────────────────────────

pub fn collect_gpu_snapshot(store: &SceneGpuStore) -> GpuSnapshot {
    GpuSnapshot {
        gen_writes: store.generation_write_count(),
        buffers: store
            .telemetry_gpu_buffers()
            .iter()
            .map(|(id, buf)| GpuBufferSnapshot {
                component_id: id.0,
                element_size: buf.element_size(),
                capacity: buf.capacity(),
            })
            .collect(),
        cell_gpu_states: store
            .telemetry_cells()
            .iter()
            .enumerate()
            .map(|(i, c)| match c {
                Some(state) => CellGpuSnapshot {
                    id: i as u32,
                    class: state.class,
                    row_base: state.row_base,
                    slot_base: state.slot_base,
                    slot_capacity: state.slot_capacity,
                    dirty_column_count: state.dirty_columns.len(),
                    pending_retire_count: state.pending.len(),
                    alive: true,
                },
                None => CellGpuSnapshot {
                    id: i as u32,
                    class: 0,
                    row_base: 0,
                    slot_base: 0,
                    slot_capacity: 0,
                    dirty_column_count: 0,
                    pending_retire_count: 0,
                    alive: false,
                },
            })
            .collect(),
    }
}

pub fn collect_pool_snapshot(store: &SceneGpuStore) -> PoolSnapshot {
    PoolSnapshot {
        row: store
            .telemetry_row_pools()
            .iter()
            .map(|p| PoolInfo {
                region_size: p.region_size(),
                total: p.total_regions(),
                free: p.free_count(),
            })
            .collect(),
        slot: store
            .telemetry_slot_pools()
            .iter()
            .map(|p| PoolInfo {
                region_size: p.region_size(),
                total: p.total_regions(),
                free: p.free_count(),
            })
            .collect(),
    }
}

fn collect_schema_snapshot() -> Vec<TypeSchema> {
    // Schemas are derived from TypeToken registrations, which are lazy.
    // For now return an empty list; a future iteration can maintain a
    // global registry of registered types accessible by ComponentId.
    Vec::new()
}

fn collect_cell_snapshot(id: u32, cell: &CellStorage) -> CellSnapshot {
    let rows = cell.rows_in_use();
    let capacity = cell.capacity();
    let user_cols = cell.user_column_count();

    // Pod columns: token_index maps ComponentId → user-col index.
    let pod_columns: Vec<(u32, usize)> = cell
        .token_index_slice()
        .iter()
        .map(|(cid, idx)| (cid.0, *idx))
        .collect();

    // Generic columns.
    let generic_columns: Vec<(String, usize)> = cell
        .generic_token_index_slice()
        .iter()
        .map(|(cid, _, idx)| (format!("{:?}", cid), *idx))
        .collect();

    // Pod column data: dump raw bytes per column.
    let mut pod_data: Vec<ColumnData> = Vec::new();
    for (cid, col_idx) in cell.token_index_slice() {
        let element_size = cell.column_size_pub(*col_idx + 1) as usize;
        let mut rows_hex: Vec<String> = Vec::new();
        for row in 0..rows {
            let raw = cell.column_raw_bytes(*cid).unwrap_or(&[]);
            let row_start = row as usize * element_size;
            if row_start + element_size <= raw.len() {
                rows_hex.push(hex_encode(&raw[row_start..row_start + element_size]));
            }
        }
        pod_data.push(ColumnData {
            component_id: cid.0,
            element_size,
            rows_hex,
        });
    }

    // Slot column.
    let slot_col = cell.slot_column();
    let slot_column: Vec<u32> = slot_col.iter().copied().collect();

    // Liveness mask.
    let liveness = cell.liveness();
    let liveness_words = (rows as usize + 63) / 64;
    let liveness_bits: Vec<u64> = (0..liveness_words)
        .map(|w| {
            let mut bits = 0u64;
            for b in 0..64.min(rows as usize - w * 64) {
                if liveness.is_live((w * 64 + b) as u32) {
                    bits |= 1u64 << b;
                }
            }
            bits
        })
        .collect();

    CellSnapshot {
        id,
        rows_in_use: rows,
        capacity,
        user_column_count: user_cols,
        pod_columns,
        generic_columns,
        pod_data,
        slot_column,
        liveness_bits,
        cell_type_name: String::new(),
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(hex_char(b >> 4));
        s.push(hex_char(b & 0xf));
    }
    s
}

fn hex_char(v: u8) -> char {
    if v < 10 {
        (b'0' + v) as char
    } else {
        (b'a' + v - 10) as char
    }
}

impl TelemetrySnapshot {
    /// Collect a full snapshot from GPU store + cell storage pairs.
    pub fn collect(store: &SceneGpuStore, cells: &[(CellId, &CellStorage)]) -> Self {
        let gpu = collect_gpu_snapshot(store);
        let pools = collect_pool_snapshot(store);
        let schema = collect_schema_snapshot();

        let mut cell_snapshots: Vec<CellSnapshot> = Vec::new();
        for (id, cell) in cells {
            cell_snapshots.push(collect_cell_snapshot(id.0, cell));
        }

        TelemetrySnapshot {
            cells: cell_snapshots,
            gpu,
            schema,
            pools,
        }
    }
}

// ── Expose column_size on CellStorage for telemetry ───────────────────────
// (column_size is already pub(crate) — we make it crate-visible for telemetry)

// ── HTTP response helpers ─────────────────────────────────────────────────

struct HttpResponse {
    status: &'static str,
    body: String,
}

fn json_response(data: &impl Serialize) -> HttpResponse {
    match serde_json::to_string_pretty(data) {
        Ok(body) => HttpResponse {
            status: "200 OK",
            body,
        },
        Err(e) => HttpResponse {
            status: "500 Internal Server Error",
            body: format!("{{\"error\":\"serialization failed: {}\"}}", e),
        },
    }
}

fn error_response(status: &'static str, msg: &str) -> HttpResponse {
    HttpResponse {
        status,
        body: format!("{{\"error\":\"{}\"}}", msg),
    }
}

// ── Route handlers ────────────────────────────────────────────────────────

fn handle_route(path: &str, snap: &TelemetrySnapshot) -> HttpResponse {
    match path {
        "/" | "/endpoints" => json_response(&ENDPOINTS),
        "/cells" => json_response(&snap.cells),
        "/gpu/buffers" => json_response(&snap.gpu.buffers),
        "/gpu" => json_response(&snap.gpu),
        "/pools" => json_response(&snap.pools),
        "/schema" => json_response(&snap.schema),
        "/stats" => {
            let alive = snap.cells.iter().filter(|c| c.rows_in_use > 0).count();
            json_response(&serde_json::json!({
                "cells": snap.cells.len(),
                "cells_alive": alive,
                "total_rows": snap.cells.iter().map(|c| c.rows_in_use as u64).sum::<u64>(),
                "gpu_buffers": snap.gpu.buffers.len(),
                "gen_writes": snap.gpu.gen_writes,
            }))
        }
        "/health" => HttpResponse {
            status: "200 OK",
            body: "{\"status\":\"ok\"}".into(),
        },
        _ if path.starts_with("/cells/") => {
            let rest = &path[7..];
            if let Some(cell_id_str) = rest.split('/').next() {
                if let Ok(id) = cell_id_str.parse::<u32>() {
                    if let Some(info) = snap.cells.iter().find(|c| c.id == id) {
                        // Check for sub-path: /cells/{id}/columns/{col}
                        if rest.contains("/columns/") {
                            let col_str = rest.split("/columns/").nth(1).unwrap_or("");
                            let col_idx: usize = col_str.parse().unwrap_or(usize::MAX);
                            let col = info.pod_data.iter().find(|c| c.component_id as usize == col_idx);
                            return match col {
                                Some(col) => json_response(col),
                                None => {
                                    // Try generic columns
                                    error_response("404 Not Found", &format!("column {} not found in cell {}", col_idx, id))
                                }
                            };
                        }
                        return json_response(info);
                    }
                    return error_response("404 Not Found", &format!("cell {} not found", id));
                }
            }
            error_response("400 Bad Request", "invalid path")
        }
        _ if path.starts_with("/query") => {
            // Parse query string: /query?cell=N&col=M&rows=start..end
            let query_part = path.split('?').nth(1).unwrap_or("");
            let params: std::collections::HashMap<&str, &str> = query_part
                .split('&')
                .filter_map(|kv| {
                    let mut parts = kv.splitn(2, '=');
                    Some((parts.next()?, parts.next()?))
                })
                .collect();

            let cell_id: u32 = params.get("cell").and_then(|s| s.parse().ok()).unwrap_or(u32::MAX);
            let col_id: u32 = params.get("col").and_then(|s| s.parse().ok()).unwrap_or(u32::MAX);
            let row_start: usize = params.get("start").and_then(|s| s.parse().ok()).unwrap_or(0);
            let row_end: usize = params.get("end").and_then(|s| s.parse().ok()).unwrap_or(usize::MAX);

            let cell = match snap.cells.iter().find(|c| c.id == cell_id) {
                Some(c) => c,
                None => return error_response("404 Not Found", &format!("cell {} not found", cell_id)),
            };

            let col = match cell.pod_data.iter().find(|c| c.component_id == col_id) {
                Some(c) => c,
                None => return error_response("404 Not Found", &format!("column {} not found in cell {}", col_id, cell_id)),
            };

            let filtered: Vec<&str> = col.rows_hex[row_start..row_end.min(col.rows_hex.len())]
                .iter()
                .map(|s| s.as_str())
                .collect();

            json_response(&serde_json::json!({
                "cell": cell_id,
                "component_id": col_id,
                "element_size": col.element_size,
                "row_start": row_start,
                "row_end": row_start + filtered.len(),
                "rows": filtered,
            }))
        }
        _ => error_response("404 Not Found", "unknown endpoint"),
    }
}

const ENDPOINTS: &[(&str, &str)] = &[
    ("/", "this help"),
    ("/cells", "list all cells"),
    ("/cells/{id}", "cell detail"),
    ("/cells/{id}/columns/{component_id}", "column raw data"),
    ("/gpu", "GPU store state"),
    ("/gpu/buffers", "GPU buffer info"),
    ("/pools", "row and slot pool status"),
    ("/schema", "registered type schemas"),
    ("/stats", "telemetry counters"),
    ("/query?cell=N&col=M&start=R1&end=R2", "query raw row data"),
    ("/health", "health check"),
];

// ── Connection handler ────────────────────────────────────────────────────

fn handle_connection(mut stream: TcpStream, snap: &TelemetrySnapshot) {
    let mut buf = [0u8; 4096];
    let n = match stream.read(&mut buf) {
        Ok(n) if n > 0 => n,
        _ => return,
    };
    let request = String::from_utf8_lossy(&buf[..n]);
    let path = request
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .split('?')
        .next()
        .unwrap_or("/");

    let response = handle_route(path, snap);
    let header = format!(
        "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status,
        response.body.len(),
    );
    let mut resp = header.into_bytes();
    resp.extend_from_slice(response.body.as_bytes());
    let _ = stream.write_all(&resp);
}

// ── Server ────────────────────────────────────────────────────────────────

/// Background HTTP server that serves SceneDB telemetry snapshots.
///
/// The main thread pushes snapshots via [`push_snapshot`], and the server
/// thread serves the latest snapshot to HTTP clients on the given port.
pub struct TelemetryServer {
    handle: Option<JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
    latest: Arc<Mutex<Option<TelemetrySnapshot>>>,
}

impl TelemetryServer {
    /// Start the telemetry HTTP server on `port` in a background thread.
    pub fn start(port: u16) -> std::io::Result<Self> {
        let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
        socket.set_reuse_address(true)?;
        let addr = socket2::SockAddr::from(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port));
        socket.bind(&addr)?;
        socket.listen(128)?;
        socket.set_nonblocking(false)?;

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_flag = Arc::clone(&shutdown);
        let latest: Arc<Mutex<Option<TelemetrySnapshot>>> = Arc::new(Mutex::new(None));
        let latest_server = Arc::clone(&latest);

        let handle = thread::Builder::new()
            .name("scenedb-telemetry".into())
            .spawn(move || {
                loop {
                    if shutdown_flag.load(Ordering::Relaxed) {
                        break;
                    }
                    match socket.accept() {
                        Ok((conn, _)) => {
                            let stream: TcpStream = conn.into();
                            let snap_guard = latest_server.lock().unwrap();
                            if let Some(ref snap) = *snap_guard {
                                handle_connection(stream, snap);
                            }
                        }
                        Err(_) => break,
                    }
                }
            })?;

        Ok(Self {
            handle: Some(handle),
            shutdown,
            latest,
        })
    }

    /// Push a fresh snapshot for the server to serve.
    /// Call this from the main thread at frame boundaries or on demand.
    pub fn push_snapshot(&self, snap: TelemetrySnapshot) {
        *self.latest.lock().unwrap() = Some(snap);
    }

    /// Signal the server thread to shut down and wait for it.
    pub fn shutdown(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for TelemetryServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}
