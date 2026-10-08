use crate::{planner::Plan, protocol::FarmSnapshot};
use anyhow::Result;
use rusqlite::{Connection, params};
use std::path::Path;

pub struct Store {
    connection: Connection,
}
impl Store {
    pub fn open(directory: &Path) -> Result<Self> {
        std::fs::create_dir_all(directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
        }
        let connection = Connection::open(directory.join("optimizer.db"))?;
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS snapshots(id INTEGER PRIMARY KEY, save_id TEXT NOT NULL, player_id TEXT NOT NULL, captured_at INTEGER NOT NULL, data TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS snapshots_identity ON snapshots(save_id,player_id,captured_at);
            CREATE TABLE IF NOT EXISTS plans(id TEXT PRIMARY KEY, save_id TEXT NOT NULL, player_id TEXT NOT NULL, snapshot_at INTEGER NOT NULL, created_at INTEGER NOT NULL, data TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS plans_identity ON plans(save_id,player_id,created_at);")?;
        Ok(Self { connection })
    }
    pub fn save_snapshot(&mut self, s: &FarmSnapshot) -> Result<()> {
        self.connection.execute(
            "INSERT INTO snapshots(save_id,player_id,captured_at,data) VALUES(?1,?2,?3,?4)",
            params![
                s.save_id,
                s.player_id,
                s.captured_at_unix_ms,
                serde_json::to_string(s)?
            ],
        )?;
        Ok(())
    }
    pub fn save_plan(&mut self, p: &Plan) -> Result<()> {
        self.connection.execute("INSERT INTO plans(id,save_id,player_id,snapshot_at,created_at,data) VALUES(?1,?2,?3,?4,?5,?6)",params![p.id,p.save_id,p.player_id,p.snapshot_at_unix_ms,chrono::Utc::now().timestamp_millis(),serde_json::to_string(p)?])?;
        Ok(())
    }
    pub fn latest_snapshot(&self) -> Result<Option<FarmSnapshot>> {
        let mut q = self
            .connection
            .prepare("SELECT data FROM snapshots ORDER BY id DESC LIMIT 1")?;
        let mut rows = q.query([])?;
        Ok(rows
            .next()?
            .map(|r| r.get::<_, String>(0))
            .transpose()?
            .map(|s| serde_json::from_str(&s))
            .transpose()?)
    }
    pub fn latest_plan(&self, save: &str, player: &str) -> Result<Option<Plan>> {
        let mut q=self.connection.prepare("SELECT data FROM plans WHERE save_id=?1 AND player_id=?2 ORDER BY created_at DESC,rowid DESC LIMIT 1")?;
        let mut rows = q.query(params![save, player])?;
        Ok(rows
            .next()?
            .map(|r| r.get::<_, String>(0))
            .transpose()?
            .map(|s| serde_json::from_str(&s))
            .transpose()?)
    }
    pub fn snapshot_at(
        &self,
        save: &str,
        player: &str,
        at: i64,
        request_id: &str,
    ) -> Result<Option<FarmSnapshot>> {
        if request_id.is_empty() {
            return Ok(None);
        }
        let mut q=self.connection.prepare("SELECT data FROM snapshots WHERE save_id=?1 AND player_id=?2 AND captured_at=?3 AND json_extract(data,'$.refresh_request_id')=?4 ORDER BY id DESC LIMIT 1")?;
        let mut rows = q.query(params![save, player, at, request_id])?;
        Ok(rows
            .next()?
            .map(|r| r.get::<_, String>(0))
            .transpose()?
            .map(|s| serde_json::from_str(&s))
            .transpose()?)
    }
}
