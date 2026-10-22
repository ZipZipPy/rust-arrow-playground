//! Splitting the id range into COPY chunks that workers pull one at a time.

use std::sync::atomic::{AtomicU64, Ordering};

/// A contiguous run of ids loaded by one `COPY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chunk {
    pub first_id: i64,
    pub len: u64,
}

impl Chunk {
    pub fn ids(self) -> impl Iterator<Item = i64> {
        self.first_id..self.first_id + self.len as i64
    }
}

/// `rows` ids starting at `first_id`, cut into chunks of `chunk_size`
/// (the last one may be shorter). The layout depends only on these three
/// numbers, which is what makes seeding independent of the worker count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkPlan {
    first_id: i64,
    rows: u64,
    chunk_size: u64,
}

impl ChunkPlan {
    pub fn new(first_id: i64, rows: u64, chunk_size: u64) -> Self {
        assert!(chunk_size > 0, "chunk size must be positive");
        Self {
            first_id,
            rows,
            chunk_size,
        }
    }

    pub fn chunk_count(&self) -> u64 {
        self.rows.div_ceil(self.chunk_size)
    }

    pub fn chunk(&self, index: u64) -> Option<Chunk> {
        let start = index.checked_mul(self.chunk_size)?;
        (start < self.rows).then(|| Chunk {
            first_id: self.first_id + start as i64,
            len: self.chunk_size.min(self.rows - start),
        })
    }

    pub fn queue(&self) -> ChunkQueue {
        ChunkQueue {
            plan: *self,
            next: AtomicU64::new(0),
        }
    }
}

/// Hands out each chunk of a plan exactly once, to whichever worker asks.
#[derive(Debug)]
pub struct ChunkQueue {
    plan: ChunkPlan,
    next: AtomicU64,
}

impl ChunkQueue {
    pub fn next(&self) -> Option<Chunk> {
        self.plan.chunk(self.next.fetch_add(1, Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_cover_every_id_once() {
        let plan = ChunkPlan::new(101, 1_050, 250);
        assert_eq!(plan.chunk_count(), 5);
        let queue = plan.queue();
        let chunks: Vec<Chunk> = std::iter::from_fn(|| queue.next()).collect();
        assert_eq!(chunks.len(), 5);
        assert_eq!(
            chunks[0],
            Chunk {
                first_id: 101,
                len: 250
            }
        );
        assert_eq!(
            chunks[4],
            Chunk {
                first_id: 1101,
                len: 50
            }
        );
        let ids: Vec<i64> = chunks.iter().flat_map(|c| c.ids()).collect();
        assert_eq!(ids, (101..1151).collect::<Vec<_>>());
        assert_eq!(queue.next(), None, "an exhausted queue stays exhausted");
    }

    #[test]
    fn empty_plan_has_no_chunks() {
        let plan = ChunkPlan::new(1, 0, 10);
        assert_eq!(plan.chunk_count(), 0);
        assert_eq!(plan.queue().next(), None);
    }
}
