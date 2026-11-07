//! The retention policy (PLAN.md §5.3): which Clips to evict so History
//! stays within its limits. Pinned Clips are never evicted; of the rest the
//! least recently used go first.

/// History's limits. `None` means no limit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Limits {
    /// The most Clips History holds, pinned ones included.
    pub max_clips: Option<usize>,
    /// How long, in milliseconds, an unpinned Clip lives after its last use.
    pub max_age_ms: Option<i64>,
    /// The largest Clip kept, in bytes.
    pub max_clip_bytes: Option<u64>,
    /// The most bytes History holds, pinned Clips included.
    pub max_total_bytes: Option<u64>,
}

/// What the policy needs to know about one Clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clip {
    pub id: i64,
    /// Unix time in milliseconds of the latest copy or Restore.
    pub last_used_at: i64,
    pub pinned: bool,
    /// Total bytes of all its representations.
    pub size: u64,
}

/// The ids of the Clips to evict from `clips` at `now` (Unix ms), oldest
/// first. Clips too large or too old go regardless; then the oldest
/// unpinned Clips go until the count and total size fit. Pinned Clips stay
/// even when they alone exceed a limit.
pub fn evict(clips: &[Clip], limits: &Limits, now: i64) -> Vec<i64> {
    let mut unpinned: Vec<&Clip> = clips.iter().filter(|c| !c.pinned).collect();
    unpinned.sort_by_key(|c| (c.last_used_at, c.id));
    let too_big = |c: &Clip| limits.max_clip_bytes.is_some_and(|max| c.size > max);
    let too_old = |c: &Clip| {
        limits
            .max_age_ms
            .is_some_and(|max| now.saturating_sub(c.last_used_at) > max)
    };
    let mut count = clips.len();
    let mut total: u64 = clips.iter().map(|c| c.size).sum();
    let over = |count: usize, total: u64| {
        limits.max_clips.is_some_and(|max| count > max)
            || limits.max_total_bytes.is_some_and(|max| total > max)
    };
    // Too big or too old go first, so the room they free counts below.
    let (mut evicted, kept): (Vec<&Clip>, Vec<&Clip>) =
        unpinned.into_iter().partition(|c| too_big(c) || too_old(c));
    for clip in &evicted {
        count -= 1;
        total -= clip.size;
    }
    for clip in kept {
        if !over(count, total) {
            break;
        }
        evicted.push(clip);
        count -= 1;
        total -= clip.size;
    }
    evicted.sort_by_key(|c| (c.last_used_at, c.id));
    evicted.into_iter().map(|c| c.id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 24 * 60 * 60 * 1000;
    const NOW: i64 = 100 * DAY;

    /// A Clip last used `age` ms before [`NOW`].
    fn clip(id: i64, age: i64, size: u64) -> Clip {
        Clip {
            id,
            last_used_at: NOW - age,
            pinned: false,
            size,
        }
    }

    fn pinned(id: i64, age: i64, size: u64) -> Clip {
        Clip {
            pinned: true,
            ..clip(id, age, size)
        }
    }

    #[test]
    fn retention_table() {
        let none = Limits::default();
        let clips = |n| Limits {
            max_clips: Some(n),
            ..none
        };
        let age = |days| Limits {
            max_age_ms: Some(days * DAY),
            ..none
        };
        let per_clip = |bytes| Limits {
            max_clip_bytes: Some(bytes),
            ..none
        };
        let total = |bytes| Limits {
            max_total_bytes: Some(bytes),
            ..none
        };
        #[rustfmt::skip]
        let table: Vec<(&str, Vec<Clip>, Limits, Vec<i64>)> = vec![
            ("no limits keep everything",
             vec![clip(1, 90 * DAY, 1 << 40), clip(2, 0, 1)], none, vec![]),
            ("within max_clips nothing goes",
             vec![clip(1, 2, 1), clip(2, 1, 1)], clips(2), vec![]),
            ("over max_clips the least recently used go first",
             vec![clip(1, 1, 1), clip(2, 3, 1), clip(3, 2, 1), clip(4, 0, 1)], clips(2), vec![2, 3]),
            ("max_clips 0 empties History",
             vec![clip(1, 1, 1), clip(2, 0, 1)], clips(0), vec![1, 2]),
            ("pinned Clips count toward max_clips but stay",
             vec![pinned(1, 9, 1), clip(2, 2, 1), clip(3, 1, 1)], clips(2), vec![2]),
            ("pinned Clips alone over max_clips evict every other",
             vec![pinned(1, 9, 1), pinned(2, 8, 1), clip(3, 0, 1)], clips(1), vec![3]),
            ("a Clip last used longer ago than max_age goes",
             vec![clip(1, 31 * DAY, 1), clip(2, 29 * DAY, 1)], age(30), vec![1]),
            ("a Clip exactly max_age old stays",
             vec![clip(1, 30 * DAY, 1)], age(30), vec![]),
            ("an old pinned Clip stays",
             vec![pinned(1, 365 * DAY, 1)], age(30), vec![]),
            ("a Clip over max_clip goes however new",
             vec![clip(1, 5, 10), clip(2, 0, 11)], per_clip(10), vec![2]),
            ("a pinned Clip over max_clip stays",
             vec![pinned(1, 0, 11)], per_clip(10), vec![]),
            ("over max_total the oldest go until it fits",
             vec![clip(1, 3, 4), clip(2, 2, 4), clip(3, 1, 4)], total(8), vec![1]),
            ("max_total counts pinned bytes",
             vec![pinned(1, 9, 6), clip(2, 2, 2), clip(3, 1, 2)], total(8), vec![2]),
            ("pinned bytes alone over max_total evict every other",
             vec![pinned(1, 9, 10), clip(2, 1, 1)], total(8), vec![2]),
            ("eviction for size frees room for the count",
             vec![clip(1, 2, 1), clip(2, 1, 99), clip(3, 0, 1)],
             Limits { max_clips: Some(2), max_clip_bytes: Some(10), ..none }, vec![2]),
            ("equal times go lowest id first",
             vec![clip(2, 1, 1), clip(1, 1, 1)], clips(1), vec![1]),
        ];
        for (name, clips, limits, expected) in table {
            assert_eq!(evict(&clips, &limits, NOW), expected, "{name}");
        }
    }
}
