//! # LSEQ
//!
//! An LSEQ tree is a CRDT for storing sequences of data (Strings, ordered lists).
//! It provides an efficient view of the stored sequence, with fast index, insertion and deletion
//! operations.
//!
//! LSEQ [1] is a member of the LOGOOT [2] family of algorithms for CRDT sequences. The major difference
//! with LOGOOT is in the _allocation strategy_ that LSEQ employs to handle insertions.
//!
//! Internally, LSEQ views the sequence as the leaves of an ordered, exponential tree. An
//! exponential tree is a tree where the number of childen grows exponentially with the depth of a
//! node. For LSEQ, each layer of the tree doubles the available space. Each child is numbered from
//! 0..2^(3+depth). The first and last child of a node cannot be turned into leaves.
//!
//! The path from the root of a tree to a leaf is called the _identifier_ of an element.
//!
//! The major challenge for LSEQs is the question of generating new identifiers for insertions.
//!
//! If we have the sequence of ordered pairs of identifiers and values `[ ix1: a , ix2: b , ix3: c ]`,
//! and we want to insert `d` at the second position, we must find an identifer ix4 such that
//! ix1 < ix4 < ix2. This ensures that every site will insert d in the same relative position in
//! the sequence even if they dont have ix2 or ix1 yet. The [`IdentGen`] encapsulates this identifier
//! generation, and ensures that the result is always between the two provided bounds.
//!
//! LSEQ is a CmRDT, to guarantee convergence it must see every operation. It also requires that
//! they are delivered in a _causal_ order. Every deletion _must_ be applied _after_ it's
//! corresponding insertion. To guarantee this property, use a causality barrier.
//!
//! [1] B. Nédelec, P. Molli, A. Mostefaoui, and E. Desmontils,
//! “LSEQ: an adaptive structure for sequences in distributed collaborative editing,”
//! in Proceedings of the 2013 ACM symposium on Document engineering - DocEng ’13,
//! Florence, Italy, 2013, p. 37, doi: 10.1145/2494266.2494278.
//!
//! [2] S. Weiss, P. Urso, and P. Molli,
//! “Logoot: A Scalable Optimistic Replication Algorithm for Collaborative Editing on P2P Networks,”
//! in 2009 29th IEEE International Conference on Distributed Computing Systems,
//! Montreal, Quebec, Canada, Jun. 2009, pp. 404–412, doi: 10.1109/ICDCS.2009.75.

/// Contains the implementation of the exponential tree for LSeq
pub mod ident;

use ident::*;
use serde::{Deserialize, Serialize};

use crate::traits::CmRDT;

use crate::vclock::Dot;

// SiteId can be generalized to any A if there is a way to generate a single invalid actor id at every site
// Currently we rely on every site using the Id 0 for that purpose.

/// Actor type for LSEQ sites.
#[derive(Debug, Hash, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SiteId(u32);

impl SiteId {
    /// Create a `SiteId` from a `u32`.
    pub fn new(id: u32) -> Self {
        SiteId(id)
    }
}

impl Default for SiteId {
    fn default() -> Self {
        SiteId(0)
    }
}

/// An `Entry` to the LSEQ consists of:
#[derive(Debug, Clone)]
pub struct Entry<T, A: Ident> {
    /// The identifier of the entry.
    pub id: Identifier<A>,
    /// The site id of the entry.
    pub dot: Dot<A>,
    /// The element for the entry.
    pub c: T,
}

/// As described in the module documentation:
///
/// An LSEQ tree is a CRDT for storing sequences of data (Strings, ordered lists).
/// It provides an efficient view of the stored sequence, with fast index, insertion and deletion
/// operations.
pub struct LSeq<T, A: Ident> {
    seq: Vec<Entry<T, A>>,
    gen: IdentGen<A>,
    dot: Dot<A>,
}

/// Operations that can be performed on an LSeq tree
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum Op<T, A: Ident> {
    /// Insert an element
    Insert {
        /// Identifier to insert at
        #[serde(flatten)]
        id: Identifier<A>,
        /// clock of site that issued insertion
        dot: Dot<A>,
        /// Element to insert
        c: T,
    },
    /// Delete an element
    Delete {
        /// The original clock information of the insertion we're removing
        remote: Dot<A>,
        #[serde(flatten)]
        /// Identifier to remove
        id: Identifier<A>,
        /// id of site that issued delete
        dot: Dot<A>,
    },
}

impl<T, A: Ident> LSeq<T, A> {
    /// Create an LSEQ for the empty string
    pub fn new(id: A) -> Self {
        LSeq {
            seq: Vec::new(),
            gen: IdentGen::new(id.clone()),
            dot: Dot::new(id, 0),
        }
    }

    /// Insert an identifier and value in the LSEQ
    pub fn insert(&mut self, ix: Identifier<A>, dot: Dot<A>, c: T) {
        // Inserts only have an impact if the identifier is in the tree
        if let Err(res) = self.seq.binary_search_by(|e| e.id.cmp(&ix)) {
            self.seq.insert(res, Entry { id: ix, dot, c });
        }
    }

    /// Remove an identifier from the LSEQ
    pub fn delete(&mut self, ix: Identifier<A>) {
        // Deletes only have an effect if the identifier is already in the tree
        if let Ok(i) = self.seq.binary_search_by(|e| e.id.cmp(&ix)) {
            self.seq.remove(i);
        }
    }

    /// Perform a local insertion of an element at a given position.
    /// If `ix` is greater than the length of the LSeq then it is appended to the end.
    pub fn insert_index(&mut self, ix: usize, c: T) -> Op<T, A>
    where
        T: Clone,
    {
        let lower = self.gen.lower();
        let upper = self.gen.upper();

        // If we're inserting past the length of the LSEQ then it's the same as appending.
        let ix_ident = if self.seq.len() <= ix {
            let prev = self
                .seq
                .last()
                .map(|Entry { id, .. }| id)
                .unwrap_or_else(|| &lower);
            self.gen.alloc(prev, &upper)
        } else {
            // To insert an element at position ix, we want it to appear in the sequence between
            // ix - 1 and ix. To do this, retreive each bound defaulting to the lower and upper
            // bounds respectively if they are not found.
            let prev = match ix.checked_sub(1) {
                Some(i) => &self.seq.get(i).unwrap().id,
                None => &lower,
            };
            let next = self
                .seq
                .get(ix)
                .map(|Entry { id, .. }| id)
                .unwrap_or(&upper);
            let a = self.gen.alloc(&prev, next);

            assert!(prev < &a);
            assert!(&a < next);
            a
        };
        let op = Op::Insert {
            id: ix_ident,
            dot: self.dot.clone(),
            c,
        };
        self.dot.counter += 1;
        self.apply(op.clone());
        op
    }

    /// Perform a local deletion at ix.
    ///
    /// If `ix` is out of bounds, i.e. `ix > self.len()`, then
    /// the `Op` is not performed and `None` is returned.
    pub fn delete_index(&mut self, ix: usize) -> Option<Op<T, A>>
    where
        T: Clone,
    {
        if ix >= self.seq.len() {
            return None;
        }

        let data = self.seq[ix].clone();

        let op = Op::Delete {
            id: data.id,
            remote: data.dot,
            dot: self.dot.clone(),
        };

        self.dot.counter += 1;
        self.apply(op.clone());

        Some(op)
    }

    /// Get the length of the LSEQ.
    pub fn len(&self) -> usize {
        self.seq.len()
    }

    /// Check if the LSEQ is empty.
    pub fn is_empty(&self) -> bool {
        self.seq.is_empty()
    }

    /// Get the elements represented by the LSEQ.
    pub fn iter(&self) -> impl Iterator<Item = &T> + '_ {
        self.seq.iter().map(|Entry { c, .. }| c)
    }

    /// Access the internal representation of the LSEQ
    pub fn raw_entries(&self) -> &Vec<Entry<T, A>> {
        &self.seq
    }
}

impl<T, A: Ident> CmRDT for LSeq<T, A> {
    type Op = Op<T, A>;
    /// Apply an operation to an LSeq instance.
    ///
    /// If the operation is an insert and the identifier is **already** present in the LSEQ instance
    /// the result is a no-op
    ///
    /// If the operation is a delete and the identifier is **not** present in the LSEQ instance the
    /// result is a no-op
    fn apply(&mut self, op: Self::Op) {
        match op {
            Op::Insert { id, dot, c } => self.insert(id, dot, c),
            Op::Delete { id, .. } => self.delete(id),
        }
    }
}
