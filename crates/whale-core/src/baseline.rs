use chrono::Utc;
use whale_protocol::{ClientBaseline, GlobalSnapshot, UsageDelta};

use crate::subtract_totals;

pub fn baseline_from_snapshot(snapshot: &GlobalSnapshot) -> ClientBaseline {
    ClientBaseline {
        epoch: snapshot.epoch.clone(),
        sequence: snapshot.sequence,
        captured_at: snapshot.generated_at,
        totals: snapshot.all_time.clone(),
    }
}

pub fn delta_from_baseline(baseline: &ClientBaseline, snapshot: &GlobalSnapshot) -> UsageDelta {
    if baseline.epoch != snapshot.epoch {
        return incompatible(baseline, snapshot, "server_epoch_changed");
    }
    if snapshot.sequence < baseline.sequence {
        return incompatible(baseline, snapshot, "sequence_regressed");
    }
    UsageDelta {
        compatible: true,
        reason: None,
        elapsed_seconds: snapshot
            .generated_at
            .signed_duration_since(baseline.captured_at)
            .num_seconds()
            .max(0),
        from_sequence: baseline.sequence,
        to_sequence: snapshot.sequence,
        totals: subtract_totals(&snapshot.all_time, &baseline.totals),
    }
}

fn incompatible(baseline: &ClientBaseline, snapshot: &GlobalSnapshot, reason: &str) -> UsageDelta {
    UsageDelta {
        compatible: false,
        reason: Some(reason.to_string()),
        elapsed_seconds: Utc::now()
            .signed_duration_since(baseline.captured_at)
            .num_seconds()
            .max(0),
        from_sequence: baseline.sequence,
        to_sequence: snapshot.sequence,
        totals: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whale_protocol::UsageTotals;

    #[test]
    fn rejects_a_new_epoch() {
        let mut snapshot = GlobalSnapshot::empty("a", "Asia/Shanghai");
        snapshot.all_time = UsageTotals {
            requests: 2,
            ..UsageTotals::default()
        };
        let baseline = baseline_from_snapshot(&snapshot);
        snapshot.epoch = "b".into();
        let delta = delta_from_baseline(&baseline, &snapshot);
        assert!(!delta.compatible);
        assert_eq!(delta.reason.as_deref(), Some("server_epoch_changed"));
    }
}
