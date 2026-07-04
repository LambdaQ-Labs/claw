//! claw-cdb — the code-as-database (WS-B).
//!
//! Source is not text: it is a content-addressed store of definitions.
//! Identity = blake3 hash of content; names are mutable pointers to hashes,
//! so rename is an O(1) metadata write and callers (which reference hashes,
//! never names) can never break.
//!
//! `candidates(type)` is the load-bearing query: it returns only real,
//! in-scope definitions whose type unifies with the request. The constraint
//! server masks generation against it — which is what makes API
//! hallucination structurally impossible rather than merely detectable.
//!
//! Spec: docs/p2-spec.md §1.

use claw_core::{unify, Def, Hash, Subst, Type};
use rusqlite::{params, Connection};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CdbError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("serialization: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("unknown name: {0}")]
    UnknownName(String),
    #[error("unknown hash: {0}")]
    UnknownHash(Hash),
}

pub type Result<T> = std::result::Result<T, CdbError>;

/// A candidate returned by the type-directed query: a real, in-scope
/// definition whose type unifies with the requested type.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub name: String,
    pub hash: Hash,
    pub ty: Type,
    pub deprecated: bool,
    /// The substitution under which this candidate's type matched the query.
    pub subst: Subst,
}

pub struct Cdb {
    conn: Connection,
}

impl Cdb {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS definitions (
                 hash      TEXT PRIMARY KEY,
                 def_json  TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS names (
                 name TEXT PRIMARY KEY,
                 hash TEXT NOT NULL REFERENCES definitions(hash)
             );
             CREATE TABLE IF NOT EXISTS edges (
                 caller TEXT NOT NULL REFERENCES definitions(hash),
                 callee TEXT NOT NULL REFERENCES definitions(hash),
                 PRIMARY KEY (caller, callee)
             );
             CREATE INDEX IF NOT EXISTS idx_edges_callee ON edges(callee);",
        )?;
        Ok(Cdb { conn })
    }

    // ------------------------------------------------------------------
    // Edit API (docs/p2-spec.md §1.5)
    // ------------------------------------------------------------------

    /// Insert a definition (idempotent — same content, same hash, no-op).
    /// Records its dependency edges. Returns the content hash.
    pub fn put(&mut self, def: &Def) -> Result<Hash> {
        let hash = def.hash();
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO definitions (hash, def_json) VALUES (?1, ?2)",
            params![hash.0, serde_json::to_string(def)?],
        )?;
        for dep in def.deps() {
            tx.execute(
                "INSERT OR IGNORE INTO edges (caller, callee) VALUES (?1, ?2)",
                params![hash.0, dep.0],
            )?;
        }
        tx.commit()?;
        Ok(hash)
    }

    /// Record a call-graph edge caller → callee directly (both by hash).
    /// Used when bodies reference dependencies by name rather than by
    /// content hash (e.g. ingested real code, where a def and its mutual
    /// recursion partner can't both be content-hashed first).
    pub fn add_edge(&self, caller: &Hash, callee: &Hash) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO edges (caller, callee) VALUES (?1, ?2)",
            params![caller.0, callee.0],
        )?;
        Ok(())
    }

    /// Point a name at a hash. Create, rename-target, or retarget —
    /// all the same O(1) metadata write.
    pub fn bind(&self, name: &str, hash: &Hash) -> Result<()> {
        let n = self.conn.execute(
            "INSERT INTO names (name, hash) VALUES (?1, ?2)
             ON CONFLICT(name) DO UPDATE SET hash = excluded.hash",
            params![name, hash.0],
        )?;
        debug_assert!(n > 0);
        Ok(())
    }

    /// Rename: move a name to a new name. O(1); no definition changes,
    /// no caller changes (callers reference hashes).
    pub fn rename(&self, from: &str, to: &str) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE names SET name = ?2 WHERE name = ?1",
            params![from, to],
        )?;
        if n == 0 {
            return Err(CdbError::UnknownName(from.to_string()));
        }
        Ok(())
    }

    pub fn remove_name(&self, name: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM names WHERE name = ?1", params![name])?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Query API (docs/p2-spec.md §1.4)
    // ------------------------------------------------------------------

    pub fn resolve(&self, name: &str) -> Result<Hash> {
        // Map ONLY "no such row" to the domain not-found error; a locked or
        // corrupt DB must surface as a real error, not masquerade as an
        // absent symbol (the invariant this store exists to uphold).
        match self.conn.query_row(
            "SELECT hash FROM names WHERE name = ?1",
            params![name],
            |r| r.get::<_, String>(0),
        ) {
            Ok(h) => Ok(Hash(h)),
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                Err(CdbError::UnknownName(name.to_string()))
            }
            Err(e) => Err(CdbError::Sqlite(e)),
        }
    }

    pub fn get(&self, hash: &Hash) -> Result<Def> {
        let json: String = match self.conn.query_row(
            "SELECT def_json FROM definitions WHERE hash = ?1",
            params![hash.0],
            |r| r.get(0),
        ) {
            Ok(j) => j,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                return Err(CdbError::UnknownHash(hash.clone()))
            }
            Err(e) => return Err(CdbError::Sqlite(e)),
        };
        Ok(serde_json::from_str(&json)?)
    }

    /// Every bound name in scope. (Scopes/modules arrive with the real
    /// compiler; MVP treats the store as one flat scope.)
    pub fn symbols(&self) -> Result<Vec<(String, Hash)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name, hash FROM names ORDER BY name")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, Hash(r.get::<_, String>(1)?)))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn contains_hash(&self, hash: &Hash) -> Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM definitions WHERE hash = ?1",
            params![hash.0],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// The heart of the CDB: every named, in-scope definition whose type
    /// unifies with `query`. Type vars in the query bind to anything, so
    /// `Nat, Nat -> a` finds both `checkedSub : Nat, Nat -> Result Nat MathErr`
    /// and `add : Nat, Nat -> Nat`.
    pub fn candidates(&self, query: &Type) -> Result<Vec<Candidate>> {
        let mut out = Vec::new();
        for (name, hash) in self.symbols()? {
            let def = self.get(&hash)?;
            // Freshen the candidate's type vars into a disjoint namespace so
            // a shared var name (`a` in both query and def) can't capture and
            // wrongly reject a legal polymorphic candidate.
            let cand_ty = claw_core::freshen(&def.ty, "$c.");
            if let Some(subst) = unify(query, &cand_ty) {
                out.push(Candidate {
                    name,
                    hash,
                    ty: def.ty.clone(),
                    deprecated: def.deprecated,
                    subst,
                });
            }
        }
        Ok(out)
    }

    /// Who references this definition?
    pub fn callers(&self, hash: &Hash) -> Result<Vec<Hash>> {
        let mut stmt = self
            .conn
            .prepare("SELECT caller FROM edges WHERE callee = ?1")?;
        let rows = stmt
            .query_map(params![hash.0], |r| Ok(Hash(r.get::<_, String>(0)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// What does this definition reference?
    pub fn deps(&self, hash: &Hash) -> Result<Vec<Hash>> {
        let mut stmt = self
            .conn
            .prepare("SELECT callee FROM edges WHERE caller = ?1")?;
        let rows = stmt
            .query_map(params![hash.0], |r| Ok(Hash(r.get::<_, String>(0)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use claw_core::{Expr, Lit};

    fn named(n: &str) -> Type {
        Type::Named(n.into())
    }

    fn nat_lit(n: i64) -> Def {
        Def::new(Expr::Lit(Lit::Int(n)), named("Nat"))
    }

    /// checkedSub : Nat, Nat -> Result Nat MathErr
    fn checked_sub() -> Def {
        Def::new(
            Expr::Lam {
                params: vec!["a".into(), "b".into()],
                body: Box::new(Expr::Var("a".into())), // body irrelevant to these tests
            },
            Type::Fn(
                vec![named("Nat"), named("Nat")],
                Box::new(Type::App(
                    "Result".into(),
                    vec![named("Nat"), named("MathErr")],
                )),
            ),
        )
    }

    #[test]
    fn put_bind_resolve_roundtrip() {
        let mut db = Cdb::in_memory().unwrap();
        let def = nat_lit(42);
        let h = db.put(&def).unwrap();
        db.bind("answer", &h).unwrap();
        assert_eq!(db.resolve("answer").unwrap(), h);
        assert_eq!(db.get(&h).unwrap(), def);
    }

    #[test]
    fn put_is_idempotent() {
        let mut db = Cdb::in_memory().unwrap();
        let h1 = db.put(&nat_lit(1)).unwrap();
        let h2 = db.put(&nat_lit(1)).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(db.symbols().unwrap().len(), 0, "no names bound yet");
    }

    #[test]
    fn add_edge_builds_the_call_graph_by_hash() {
        // Bodies that reference deps by NAME (ingested real code) can't be
        // content-hashed into Ref edges up front; add_edge links them.
        let mut db = Cdb::in_memory().unwrap();
        let callee_h = db.put(&nat_lit(7)).unwrap();
        db.bind("helper", &callee_h).unwrap();
        // caller body references `helper` by name (a Var, not a Ref)
        let caller = Def::new(
            Expr::App {
                func: Box::new(Expr::Var("helper".into())),
                args: vec![],
            },
            named("Nat"),
        );
        let caller_h = db.put(&caller).unwrap();
        db.bind("useHelper", &caller_h).unwrap();
        // put recorded no edge (no Ref in the body)
        assert!(db.deps(&caller_h).unwrap().is_empty());
        // link it explicitly
        db.add_edge(&caller_h, &callee_h).unwrap();
        assert_eq!(db.deps(&caller_h).unwrap(), vec![callee_h.clone()]);
        assert_eq!(db.callers(&callee_h).unwrap(), vec![caller_h]);
    }

    #[test]
    fn rename_is_metadata_only_and_callers_survive() {
        let mut db = Cdb::in_memory().unwrap();
        let callee = nat_lit(7);
        let callee_h = db.put(&callee).unwrap();
        db.bind("seven", &callee_h).unwrap();

        // caller references callee BY HASH
        let caller = Def::new(
            Expr::App {
                func: Box::new(Expr::Ref(callee_h.clone())),
                args: vec![],
            },
            named("Nat"),
        );
        let caller_h = db.put(&caller).unwrap();
        db.bind("useSeven", &caller_h).unwrap();

        // rename the callee
        db.rename("seven", "lucky").unwrap();

        // caller's dependency edge is untouched; resolution still works
        assert_eq!(db.deps(&caller_h).unwrap(), vec![callee_h.clone()]);
        assert_eq!(db.callers(&callee_h).unwrap(), vec![caller_h]);
        assert_eq!(db.resolve("lucky").unwrap(), callee_h);
        assert!(db.resolve("seven").is_err(), "old name gone");
    }

    #[test]
    fn candidates_finds_by_type_with_vars() {
        let mut db = Cdb::in_memory().unwrap();
        let sub = checked_sub();
        let sub_h = db.put(&sub).unwrap();
        db.bind("Nat.checkedSub", &sub_h).unwrap();

        let lit_h = db.put(&nat_lit(0)).unwrap();
        db.bind("zero", &lit_h).unwrap();

        // query: Nat, Nat -> a   (return type free)
        let query = Type::Fn(
            vec![named("Nat"), named("Nat")],
            Box::new(Type::Var("a".into())),
        );
        let found = db.candidates(&query).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Nat.checkedSub");
        // the substitution tells the caller what `a` became
        assert_eq!(
            found[0].subst.get("a"),
            Some(&Type::App(
                "Result".into(),
                vec![named("Nat"), named("MathErr")]
            ))
        );
    }

    #[test]
    fn candidates_excludes_nonmatching_types() {
        let mut db = Cdb::in_memory().unwrap();
        let h = db.put(&nat_lit(5)).unwrap();
        db.bind("five", &h).unwrap();
        let query = Type::Fn(vec![named("Str")], Box::new(named("Str")));
        assert!(db.candidates(&query).unwrap().is_empty());
    }

    #[test]
    fn hallucinated_symbol_is_detectable() {
        // The property the whole thesis rests on: a reference to a hash
        // that isn't in the store is mechanically visible.
        let db = Cdb::in_memory().unwrap();
        let ghost = Hash("deadbeef".repeat(8));
        assert!(!db.contains_hash(&ghost).unwrap());
    }
}
