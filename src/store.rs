use crate::model::{HourlySummary, Incident, Sample};
use anyhow::{Result, anyhow};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::{io::Write, path::Path, sync::Mutex};

pub struct Store {
    connection: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(std::time::Duration::from_secs(3))?;
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
            CREATE TABLE IF NOT EXISTS samples (at_ms INTEGER PRIMARY KEY, payload TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS incidents (id INTEGER PRIMARY KEY, started_ms INTEGER NOT NULL, ended_ms INTEGER, payload TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS incidents_started ON incidents(started_ms);
            CREATE TABLE IF NOT EXISTS hourly (hour TEXT PRIMARY KEY, samples INTEGER NOT NULL, unstable INTEGER NOT NULL, offline INTEGER NOT NULL, observed_seconds REAL NOT NULL, unstable_seconds REAL NOT NULL);
            PRAGMA user_version=1;")?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }
    fn with<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| anyhow!("History database lock was poisoned"))?;
        f(&mut connection)
    }
    pub fn record(
        &self,
        sample: &Sample,
        incident: &mut Option<Incident>,
        observed_seconds: f64,
    ) -> Result<()> {
        self.with(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute("INSERT INTO samples(at_ms,payload) VALUES (?1,?2)", params![sample.at.timestamp_millis(), serde_json::to_string(sample)?])?;
            let unstable = sample.diagnosis.severity.is_unstable();
            transaction.execute("INSERT INTO hourly VALUES (?1,1,?2,?3,?4,?5) ON CONFLICT(hour) DO UPDATE SET samples=samples+1,unstable=unstable+excluded.unstable,offline=offline+excluded.offline,observed_seconds=observed_seconds+excluded.observed_seconds,unstable_seconds=unstable_seconds+excluded.unstable_seconds",
                params![sample.at.format("%Y-%m-%dT%H:00:00Z").to_string(), unstable, sample.diagnosis.severity == crate::model::Severity::Offline, observed_seconds, if unstable { observed_seconds } else { 0.0 }])?;
            if let Some(row) = incident {
                if row.id == 0 {
                    transaction.execute("INSERT INTO incidents(started_ms,ended_ms,payload) VALUES (?1,?2,?3)", params![row.started_at.timestamp_millis(), row.ended_at.map(|v| v.timestamp_millis()), serde_json::to_string(row)?])?;
                    row.id = transaction.last_insert_rowid();
                }
                transaction.execute("UPDATE incidents SET ended_ms=?1,payload=?2 WHERE id=?3", params![row.ended_at.map(|v| v.timestamp_millis()), serde_json::to_string(row)?, row.id])?;
            }
            transaction.commit()?; Ok(())
        })
    }
    pub fn close_interrupted(&self, at: DateTime<Utc>, reason: &str) -> Result<()> {
        self.with(|connection| {
            let mut statement =
                connection.prepare("SELECT payload FROM incidents WHERE ended_ms IS NULL")?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            for payload in rows {
                let mut incident: Incident = serde_json::from_str(&payload)?;
                incident.ended_at = Some(at.max(incident.started_at));
                incident.end_reason = Some(reason.into());
                connection.execute(
                    "UPDATE incidents SET ended_ms=?1,payload=?2 WHERE id=?3",
                    params![
                        incident.ended_at.map(|v| v.timestamp_millis()),
                        serde_json::to_string(&incident)?,
                        incident.id
                    ],
                )?;
            }
            Ok(())
        })
    }
    pub fn latest(&self) -> Result<Option<Sample>> {
        self.with(|connection| {
            let payload: Option<String> = connection
                .query_row(
                    "SELECT payload FROM samples ORDER BY at_ms DESC LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            payload
                .map(|v| serde_json::from_str(&v).map_err(Into::into))
                .transpose()
        })
    }
    pub fn samples(&self, limit: usize) -> Result<Vec<Sample>> {
        self.with(|connection| {
            let mut statement =
                connection.prepare("SELECT payload FROM samples ORDER BY at_ms DESC LIMIT ?1")?;
            let mut result = Vec::new();
            for payload in
                statement.query_map([limit.min(10000) as i64], |row| row.get::<_, String>(0))?
            {
                result.push(serde_json::from_str(&payload?)?);
            }
            result.reverse();
            Ok(result)
        })
    }
    pub fn incidents(&self, limit: usize) -> Result<Vec<Incident>> {
        self.with(|connection| {
            let mut statement = connection
                .prepare("SELECT payload FROM incidents ORDER BY started_ms DESC LIMIT ?1")?;
            let mut result = Vec::new();
            for payload in
                statement.query_map([limit.min(1000) as i64], |row| row.get::<_, String>(0))?
            {
                result.push(serde_json::from_str(&payload?)?);
            }
            Ok(result)
        })
    }
    pub fn hourly(&self, limit: usize) -> Result<Vec<HourlySummary>> {
        self.with(|connection| {
            let mut statement = connection.prepare("SELECT hour,samples,unstable,offline,observed_seconds,unstable_seconds FROM hourly ORDER BY hour DESC LIMIT ?1")?;
            let rows = statement.query_map([limit.min(87600) as i64], |row| Ok(HourlySummary { hour: row.get(0)?, samples: row.get(1)?, unstable_samples: row.get(2)?, offline_samples: row.get(3)?, observed_seconds: row.get(4)?, unstable_seconds: row.get(5)? }))?;
            let mut result = rows.collect::<Result<Vec<_>, _>>()?; result.reverse(); Ok(result)
        })
    }
    pub fn prune(&self, now: DateTime<Utc>, sample_days: u32, incident_days: u32) -> Result<()> {
        self.with(|connection| {
            let transaction = connection.transaction()?;
            if sample_days > 0 {
                transaction.execute(
                    "DELETE FROM samples WHERE at_ms < ?1",
                    [(now - Duration::days(sample_days as i64)).timestamp_millis()],
                )?;
            }
            if incident_days > 0 {
                let cutoff = now - Duration::days(incident_days as i64);
                transaction.execute(
                    "DELETE FROM incidents WHERE ended_ms IS NOT NULL AND ended_ms < ?1",
                    [cutoff.timestamp_millis()],
                )?;
                transaction.execute(
                    "DELETE FROM hourly WHERE hour < ?1",
                    [cutoff.format("%Y-%m-%dT%H:00:00Z").to_string()],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
    }
    /// Stream the complete retained history. No silent row limit or large in-memory export.
    pub fn export(&self, writer: &mut impl Write, since: Option<DateTime<Utc>>) -> Result<()> {
        self.with(|connection| {
            writeln!(writer, "{}", serde_json::json!({"type":"metadata", "schema_version":1, "app":"aujitter", "version":env!("CARGO_PKG_VERSION"), "exported_at":Utc::now(), "since":since, "privacy":"Local addresses and ISP labels may be present. Review before sharing."}))?;
            let start = since.map(|at| at.timestamp_millis()).unwrap_or(i64::MIN);
            for (sql, kind) in [
                ("SELECT payload FROM samples WHERE at_ms >= ?1 ORDER BY at_ms", "sample"),
                ("SELECT payload FROM incidents WHERE ended_ms IS NULL OR ended_ms >= ?1 ORDER BY started_ms", "incident")
            ] {
                let mut statement = connection.prepare(sql)?;
                for payload in statement.query_map([start], |row| row.get::<_, String>(0))? {
                    let payload: serde_json::Value = serde_json::from_str(&payload?)?;
                    writeln!(writer, "{}", serde_json::json!({"type":kind, "value":payload}))?;
                }
            }
            let cutoff = since.map(|at| at.format("%Y-%m-%dT%H:00:00Z").to_string()).unwrap_or_default();
            let mut statement = connection.prepare("SELECT hour,samples,unstable,offline,observed_seconds,unstable_seconds FROM hourly WHERE hour >= ?1 ORDER BY hour")?;
            let rows = statement.query_map([cutoff], |row| Ok(HourlySummary { hour: row.get(0)?, samples: row.get(1)?, unstable_samples: row.get(2)?, offline_samples: row.get(3)?, observed_seconds: row.get(4)?, unstable_seconds: row.get(5)? }))?;
            for row in rows {
                writeln!(writer, "{}", serde_json::json!({"type":"hourly", "value":row?}))?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Diagnosis, Metrics, Severity, Topology};
    fn sample(at: DateTime<Utc>, severity: Severity) -> Sample {
        let diagnosis = Diagnosis {
            severity,
            ..Default::default()
        };
        Sample {
            at,
            interval_ms: 5000,
            topology: Topology::default(),
            probes: vec![],
            metrics: Metrics::default(),
            diagnosis,
            observation_gap_seconds: None,
        }
    }
    #[test]
    fn retains_incidents_after_raw_sample_expiry_and_recovers_after_restart() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("history.sqlite3");
        let at = Utc::now() - Duration::days(40);
        let store = Store::open(&path).unwrap();
        let row = sample(at, Severity::Offline);
        let mut incident = Some(Incident {
            id: 0,
            started_at: at,
            ended_at: None,
            peak_severity: Severity::Offline,
            unstable_samples: 1,
            diagnosis: row.diagnosis.clone(),
            end_reason: None,
        });
        store.record(&row, &mut incident, 5.0).unwrap();
        let id = incident.as_ref().unwrap().id;
        assert!(id > 0);
        drop(store);
        let store = Store::open(&path).unwrap();
        store.close_interrupted(at, "restart").unwrap();
        store.prune(Utc::now(), 30, 365).unwrap();
        assert!(store.samples(10).unwrap().is_empty());
        assert_eq!(store.incidents(10).unwrap()[0].id, id);
        assert_eq!(store.hourly(24).unwrap()[0].observed_seconds, 5.0);
        let mut output = Vec::new();
        store.export(&mut output, None).unwrap();
        assert!(String::from_utf8(output).unwrap().contains("incident"));
    }
}
