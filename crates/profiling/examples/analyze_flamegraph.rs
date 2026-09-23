use rusqlite::Connection;
use std::{collections::HashMap, env};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args().nth(1).expect("database path");
    let conn = Connection::open(path)?;
    let mut stmt = conn.prepare("SELECT name, COUNT(*), SUM(duration_ns), MAX(duration_ns) FROM profile_events GROUP BY name ORDER BY SUM(duration_ns) DESC LIMIT 40")?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?)))?;
    for row in rows { let (name, count, total, max) = row?; println!("{total:>14} ns total | {max:>12} ns max | {count:>8} | {name}"); }
    let mut stmt = conn.prepare("SELECT COUNT(*), AVG(duration_ns), MAX(duration_ns), SUM(duration_ns) FROM profile_events WHERE name LIKE '%frame%' OR name LIKE '%Frame%' OR name LIKE '%render%' OR name LIKE '%Render%'")?;
    let row: (i64, f64, i64, i64) = stmt.query_row([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
    println!("FRAME/RENDER count={} avg_ns={} max_ns={} total_ns={}", row.0, row.1, row.2, row.3);
    Ok(())
}
