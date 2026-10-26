use rusqlite::{Connection, OptionalExtension, params};

use super::{Graph, NodeId};

pub(super) fn drop_host(db: &Connection, node: NodeId) -> rusqlite::Result<()> {
    db.execute("DELETE FROM node_hosts WHERE node = ?1", [node.0])
        .map(drop)
}

impl Graph {
    pub fn set_host(&self, node: NodeId, address: &str) -> rusqlite::Result<()> {
        self.lock()
            .execute(
                "INSERT INTO node_hosts (node, address) SELECT id, ?2 FROM nodes WHERE id = ?1
                 ON CONFLICT (node) DO UPDATE SET address = excluded.address, unreached = NULL",
                params![node.0, address],
            )
            .map(drop)
    }

    pub fn host_of(&self, node: NodeId) -> rusqlite::Result<Option<String>> {
        self.lock()
            .query_row(
                "SELECT address FROM node_hosts WHERE node = ?1",
                [node.0],
                |row| row.get(0),
            )
            .optional()
    }

    pub fn host_reached(&self, node: NodeId) -> rusqlite::Result<()> {
        self.lock()
            .execute(
                "UPDATE node_hosts SET unreached = NULL WHERE node = ?1",
                [node.0],
            )
            .map(drop)
    }

    pub fn host_unreached(&self, node: NodeId, cause: &str) -> rusqlite::Result<()> {
        self.lock()
            .execute(
                "UPDATE node_hosts SET unreached = ?2 WHERE node = ?1",
                params![node.0, cause],
            )
            .map(drop)
    }

    pub fn unreached_hosts(&self) -> rusqlite::Result<Vec<NodeId>> {
        self.lock()
            .prepare(
                "SELECT node FROM node_hosts JOIN nodes ON nodes.id = node
                 WHERE unreached IS NOT NULL AND process_exited = 0 ORDER BY node",
            )?
            .query_map([], |row| Ok(NodeId(row.get(0)?)))?
            .collect()
    }

    pub fn claim_unreached(&self, node: NodeId) -> rusqlite::Result<bool> {
        self.lock()
            .execute(
                "UPDATE node_hosts SET unreached = NULL WHERE node = ?1 AND unreached IS NOT NULL",
                [node.0],
            )
            .map(|changed| changed == 1)
    }
}
