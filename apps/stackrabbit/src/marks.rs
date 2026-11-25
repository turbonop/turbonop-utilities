//! Marks: text a recorder put in the trace (PerfInfo `Mark` events: WPR
//! marks the start of its rundown, the profiles it recorded and the
//! recording's name; `xperf -m` adds one), as WPA's Marks table lists
//! them.

use std::path::Path;

use serde_json::{Value, json};
use turbonop_etw::guids;
use turbonop_etw::schema::{Properties, pointer_size};
use turbonop_trace::Trace;

/// PerfInfo's `Mark`.
const MARK: u8 = 34;

/// The marks of `trace` (read from `path`), in time order: each one's
/// time, processor and text.
pub fn marks(trace: &Trace, path: &Path) -> Result<Value, String> {
    let wanted =
        |provider: &turbonop_etl::Guid, op: u8| *provider == guids::PERF_INFO && op == MARK;
    let (mut all, _) = crate::reading::events(trace, path, wanted, |processor, e, mine| {
        let Some(Ok(p)) = Properties::of(e) else {
            return;
        };
        let text = p.get("Message").map_or_else(String::new, |m| {
            turbonop_etw::values::format(m, pointer_size(e))
        });
        mine.push((e.timestamp, processor, text));
    })?;
    all.sort();
    Ok(json!({
        "rows": all.iter().map(|(t, cpu, mark)| json!({
            "timeNs": trace.clock.nanos(*t),
            "cpu": cpu,
            "mark": mark,
        })).collect::<Vec<_>>(),
    }))
}
