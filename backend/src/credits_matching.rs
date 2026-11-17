//! Tail Chromaprint matching ported from hbq0405/emby-toolkit algorithm v6.
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

const MIN_SECONDS: f64 = 18.0;
const MAX_SECONDS: f64 = 300.0;
const MASKS: [u32; 4] = [0x0f0f0f0f, 0xf0f0f0f0, 0x33333333, 0xcccccccc];
const ANCHOR: usize = 3;

#[derive(Clone, Debug)]
pub(crate) struct Fingerprint {
    pub values: Vec<u32>,
    pub sample_seconds: f64,
    pub start_seconds: f64,
}

impl Fingerprint {
    fn seconds_per_value(&self) -> f64 {
        if self.values.is_empty() || !self.sample_seconds.is_finite() || self.sample_seconds <= 0.0
        {
            return 0.0;
        }
        (self.sample_seconds / self.values.len() as f64).max(0.01)
    }
}

struct Deadline(Instant);
impl Deadline {
    fn new(seconds: u64) -> Self {
        Self(Instant::now() + Duration::from_secs(seconds))
    }
    fn check(&self) -> Option<()> {
        (Instant::now() < self.0).then_some(())
    }
    fn yield_cpu(&self, index: usize, interval: usize) -> Option<()> {
        if index > 0 && index.is_multiple_of(interval) {
            std::thread::sleep(Duration::from_millis(1));
        }
        self.check()
    }
}

type AnchorIndex = HashMap<(usize, [u32; ANCHOR]), Vec<usize>>;

fn anchor(values: &[u32], offset: usize, mask: u32) -> [u32; ANCHOR] {
    [
        values[offset] & mask,
        values[offset + 1] & mask,
        values[offset + 2] & mask,
    ]
}

fn make_index(values: &[u32], upper: usize, deadline: &Deadline) -> Option<AnchorIndex> {
    let mut index = AnchorIndex::new();
    for offset in 0..upper.min(values.len().saturating_sub(ANCHOR - 1)) {
        deadline.yield_cpu(offset, 32)?;
        for (mask_index, mask) in MASKS.into_iter().enumerate() {
            index
                .entry((mask_index, anchor(values, offset, mask)))
                .or_default()
                .push(offset);
        }
    }
    Some(index)
}

fn extend(
    a: &[u32],
    b: &[u32],
    mut i: isize,
    mut j: isize,
    step: isize,
    deadline: &Deadline,
) -> Option<(usize, usize)> {
    let (mut total, mut matches, mut misses) = (0, 0, 0);
    let mut last_good = (0, 0);
    while i >= 0 && j >= 0 && (i as usize) < a.len() && (j as usize) < b.len() {
        total += 1;
        if (a[i as usize] ^ b[j as usize]).count_ones() <= 6 {
            matches += 1;
            misses = 0;
            last_good = (total, matches);
        } else {
            misses += 1;
            if misses > 40 {
                break;
            }
        }
        deadline.yield_cpu(total, 512)?;
        i += step;
        j += step;
    }
    Some(last_good)
}

fn segment(
    base: &Fingerprint,
    other: &Fingerprint,
    deadline: &Deadline,
) -> Option<(f64, f64, f64, f64)> {
    deadline.check()?;
    let (a, b) = (&base.values, &other.values);
    let spv = base.seconds_per_value().max(other.seconds_per_value());
    if a.is_empty() || b.is_empty() || spv <= 0.0 {
        return None;
    }
    let min_len = ((MIN_SECONDS / spv) as usize).max(8);
    let max_start = a
        .len()
        .min(b.len())
        .saturating_sub(min_len)
        .min((base.sample_seconds.max(other.sample_seconds) / spv) as usize);
    if max_start == 0 || a.len().min(b.len()) < ANCHOR {
        return None;
    }
    let index = make_index(b, max_start + 1, deadline)?;
    let mut best = (0, 0, 0);
    for i in 0..(a.len() - ANCHOR + 1).min(max_start + 1) {
        deadline.yield_cpu(i, 32)?;
        for (mask_index, mask) in MASKS.into_iter().enumerate() {
            let Some(hits) = index.get(&(mask_index, anchor(a, i, mask))) else {
                continue;
            };
            for &j in hits.iter().take(8) {
                deadline.check()?;
                let (right, right_matches) = extend(a, b, i as isize, j as isize, 1, deadline)?;
                let (left, left_matches) =
                    extend(a, b, i as isize - 1, j as isize - 1, -1, deadline)?;
                let total = left + right;
                if total >= min_len
                    && (left_matches + right_matches) as f64 / total as f64 >= 0.85
                    && total > best.2
                {
                    best = (i - left, j - left, total);
                }
            }
        }
    }
    (best.2 >= min_len).then(|| {
        (
            best.0 as f64 * base.seconds_per_value(),
            (best.0 + best.2) as f64 * base.seconds_per_value(),
            best.1 as f64 * other.seconds_per_value(),
            (best.1 + best.2) as f64 * other.seconds_per_value(),
        )
    })
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

/// Returns the base sample index and its absolute matching range in seconds.
/// The caller expands tail samples when the start touches
/// the eight-second window margin, then validates the full media runtime.
pub(crate) fn detect_common(samples: &[Fingerprint]) -> Option<(usize, f64, f64)> {
    detect(samples, false)
}

/// The first sample is the episode being processed. A different song shared
/// only by its neighbours must never replace or hide its own matching range.
pub(crate) fn detect_target(samples: &[Fingerprint]) -> Option<(f64, f64)> {
    detect(samples, true).map(|(_, start, end)| (start, end))
}

fn detect(samples: &[Fingerprint], target_only: bool) -> Option<(usize, f64, f64)> {
    let deadline = Deadline::new(60);
    let mut pairs = vec![vec![]; samples.len()];
    for (i, base) in samples.iter().enumerate() {
        if target_only && i != 0 {
            break;
        }
        for (j, other) in samples.iter().enumerate().skip(i + 1) {
            deadline.check()?;
            if let Some(found) = segment(base, other, &deadline) {
                let duration = (found.1 - found.0).min(found.3 - found.2);
                if (MIN_SECONDS..=MAX_SECONDS).contains(&duration) {
                    pairs[i].push(found);
                    pairs[j].push((found.2, found.3, found.0, found.1));
                }
            }
        }
    }
    let mut best: Option<(usize, usize, f64, f64)> = None;
    for (base_index, segments) in pairs.iter().enumerate() {
        if target_only && base_index != 0 {
            break;
        }
        // 不合并互不重叠的歌曲区间；在同一实际匹配范围内选择支持最多的模板。
        for candidate in segments {
            let agreed = segments
                .iter()
                .filter(|s| {
                    if target_only {
                        let overlap = s.1.min(candidate.1) - s.0.max(candidate.0);
                        overlap >= MIN_SECONDS
                            && overlap >= (s.1 - s.0).min(candidate.1 - candidate.0) * 0.75
                    } else {
                        (s.0 - candidate.0).abs() <= 2.0 && (s.1 - candidate.1).abs() <= 5.0
                    }
                })
                .collect::<Vec<_>>();
            // 三集以上时至少由两个独立参照确认，不能只因某两集正文里的
            // 背景音乐相同就忽略其他样本的否定结果。
            if agreed.len() < if samples.len() >= 3 { 2 } else { 1 } {
                continue;
            }
            // A cut or voiceover may shorten a match. Accept only the shared
            // intersection, starting at the later confirmed point, never a midpoint.
            let (start, end) = if target_only {
                (
                    agreed.iter().map(|s| s.0).max_by(f64::total_cmp)?,
                    agreed.iter().map(|s| s.1).min_by(f64::total_cmp)?,
                )
            } else {
                (
                    median(agreed.iter().map(|s| s.0).collect()),
                    median(agreed.iter().map(|s| s.1).collect()),
                )
            };
            if end - start < MIN_SECONDS {
                continue;
            }
            if best.as_ref().is_none_or(|(_, count, old_start, old_end)| {
                agreed.len() > *count || agreed.len() == *count && end - start > old_end - old_start
            }) {
                best = Some((base_index, agreed.len(), start, end));
            }
        }
    }
    best.map(|(index, _, start, end)| {
        (
            index,
            samples[index].start_seconds + start,
            samples[index].start_seconds + end,
        )
    })
}

fn template_range(
    base: &Fingerprint,
    other: &Fingerprint,
    start: f64,
    end: f64,
    deadline: &Deadline,
) -> Option<(f64, f64)> {
    deadline.check()?;
    let (a, b) = (&base.values, &other.values);
    let (base_spv, other_spv) = (base.seconds_per_value(), other.seconds_per_value());
    if a.len().min(b.len()) < ANCHOR
        || base_spv <= 0.0
        || other_spv <= 0.0
        || !start.is_finite()
        || !end.is_finite()
    {
        return None;
    }
    let min_len = ((MIN_SECONDS / base_spv.max(other_spv)) as usize).max(8);
    let template_start = (start / base_spv).round_ties_even().max(0.0) as usize;
    let template_end = ((end / base_spv).round_ties_even().max(0.0) as usize).min(a.len());
    let template_len = template_end.checked_sub(template_start)?;
    if template_len < min_len {
        return None;
    }
    let max_other_start = (b.len() - ANCHOR).min((other.sample_seconds / other_spv) as usize);
    let index = make_index(b, max_other_start + 1, deadline)?;
    let min_overlap = min_len.max((template_len as f64 * 0.75) as usize);
    let tolerance = 2.max((8.0 / base_spv) as usize);
    let (mut best, mut best_score) = (None, (0, 0, 0));
    let upper = template_end.saturating_sub(ANCHOR).max(template_start);
    for i in template_start..=upper.min(a.len() - ANCHOR) {
        deadline.yield_cpu(i, 32)?;
        for (mask_index, mask) in MASKS.into_iter().enumerate() {
            let Some(hits) = index.get(&(mask_index, anchor(a, i, mask))) else {
                continue;
            };
            for &j in hits.iter().take(8) {
                deadline.check()?;
                let (right, right_matches) = extend(a, b, i as isize, j as isize, 1, deadline)?;
                let (left, left_matches) =
                    extend(a, b, i as isize - 1, j as isize - 1, -1, deadline)?;
                let (total, matches) = (left + right, left_matches + right_matches);
                if total < min_len || matches as f64 / (total as f64) < 0.85 {
                    continue;
                }
                let (base_start, other_start) = (i - left, j - left);
                let base_end = base_start + total;
                let overlap = base_end
                    .min(template_end)
                    .saturating_sub(base_start.max(template_start));
                if overlap < min_overlap
                    || base_start > template_start + tolerance
                    || base_end < template_end.saturating_sub(tolerance)
                {
                    continue;
                }
                let score = (overlap, matches, total);
                if score > best_score {
                    best_score = score;
                    best = Some((base_start, other_start));
                }
            }
        }
    }
    let (base_start, other_start) = best?;
    let own_start = other_start
        .checked_add(template_start)?
        .checked_sub(base_start)?;
    let own_end = other_start
        .checked_add(template_end)?
        .checked_sub(base_start)?;
    (own_end > own_start && own_end <= b.len())
        .then_some((own_start as f64 * other_spv, own_end as f64 * other_spv))
}

/// Match an absolute template range against another episode's tail sample.
pub(crate) fn match_template(
    target: &Fingerprint,
    template: &Fingerprint,
    start_seconds: f64,
    end_seconds: f64,
) -> Option<(f64, f64)> {
    let start = start_seconds - template.start_seconds;
    let end = end_seconds - template.start_seconds;
    let (own_start, own_end) = template_range(template, target, start, end, &Deadline::new(20))?;
    let duration = (end - start).min(own_end - own_start);
    (MIN_SECONDS..=MAX_SECONDS).contains(&duration).then_some((
        target.start_seconds + own_start,
        target.start_seconds + own_end,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(count: usize, mut seed: u32) -> Vec<u32> {
        (0..count)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed
            })
            .collect()
    }
    fn sample(prefix: usize, common: &[u32], suffix: usize, seed: u32) -> Fingerprint {
        let mut values = noise(prefix, seed);
        values.extend_from_slice(common);
        values.extend(noise(suffix, seed.wrapping_add(71)));
        Fingerprint {
            sample_seconds: values.len() as f64 / 8.0,
            values,
            start_seconds: 1000.0,
        }
    }

    #[test]
    fn finds_shifted_tail_music_and_maps_absolute_episode_time() {
        let common = noise(400, 9001);
        let a = sample(320, &common, 320, 2);
        let b = sample(480, &common, 160, 3);
        let (base, start, end) = detect_common(&[a.clone(), b.clone()]).unwrap();
        assert_eq!((base, start, end), (0, 1040.0, 1090.0));
        assert_eq!(match_template(&b, &a, start, end), Some((1060.0, 1110.0)));
    }

    #[test]
    fn tolerates_six_bit_encoding_errors_and_short_discontinuity() {
        let common = noise(400, 9001);
        let a = sample(320, &common, 320, 2);
        let mut b = sample(480, &common, 160, 3);
        for index in 40..360 {
            if index % 4 == 0 {
                b.values[480 + index] ^= 0b11_1111;
            }
        }
        for index in 180..205 {
            b.values[480 + index] ^= u32::MAX;
        }
        assert_eq!(
            match_template(&b, &a, 1040.0, 1090.0),
            Some((1060.0, 1110.0))
        );
    }

    #[test]
    fn rejects_seven_bit_errors_and_below_eighty_five_percent_matches() {
        let common = noise(400, 9001);
        let a = sample(320, &common, 320, 2);
        let mut b = sample(480, &common, 160, 3);
        for value in &mut b.values[480..880] {
            *value ^= 0b111_1111;
        }
        assert!(match_template(&b, &a, 1040.0, 1090.0).is_none());
        b = sample(480, &common, 160, 3);
        for index in (100..135).chain(200..235) {
            b.values[480 + index] ^= u32::MAX;
        }
        assert!(match_template(&b, &a, 1040.0, 1090.0).is_none());
    }

    #[test]
    fn rejects_unrelated_short_and_overlong_common_audio() {
        assert!(
            detect_common(&[
                sample(40, &noise(160, 1), 40, 2),
                sample(40, &noise(160, 3), 40, 4)
            ])
            .is_none()
        );
        let common = noise(100, 91);
        assert!(detect_common(&[sample(80, &common, 80, 2), sample(80, &common, 80, 3)]).is_none());
        let common = noise(301, 91);
        let mut a = sample(80, &common, 80, 2);
        let mut b = sample(80, &common, 80, 3);
        a.sample_seconds = a.values.len() as f64;
        b.sample_seconds = b.values.len() as f64;
        assert!(detect_common(&[a, b]).is_none());
    }

    #[test]
    fn returns_boundary_for_caller_to_expand_before_accepting() {
        let common = noise(400, 9001);
        let a = sample(0, &common, 160, 2);
        let b = sample(16, &common, 144, 3);
        assert_eq!(detect_common(&[a, b]), Some((0, 1000.0, 1050.0)));
    }

    #[test]
    fn a_different_ending_does_not_hide_three_independently_matching_episodes() {
        let common = noise(400, 9001);
        let unrelated = sample(1200, &noise(500, 77), 160, 42);
        let a = sample(320, &common, 320, 2);
        let b = sample(480, &common, 160, 3);
        let c = sample(400, &common, 240, 4);
        assert!(detect_common(&[unrelated.clone(), a.clone(), b.clone()]).is_none());
        let samples = [unrelated, a.clone(), b, c];
        let (index, start, end) = detect_common(&samples).unwrap();
        assert_ne!(index, 0);
        let (own_start, own_end) = match_template(&a, &samples[index], start, end).unwrap();
        assert!((own_start - 1040.0).abs() <= 0.5);
        assert!((own_end - 1090.0).abs() <= 0.5);
        assert!(match_template(&samples[0], &samples[index], start, end).is_none());
    }

    #[test]
    fn unrelated_common_sections_never_produce_a_synthetic_midpoint_marker() {
        let first = noise(240, 912);
        let second = noise(240, 913);
        let mut base = sample(160, &first, 400, 17);
        base.values.extend_from_slice(&second);
        base.sample_seconds = base.values.len() as f64 / 8.0;
        let left = sample(160, &first, 640, 18);
        let right = sample(160, &second, 640, 19);
        let samples = [base, left, right];
        assert!(detect_common(&samples).is_none());
        assert!(detect_target(&samples).is_none());
    }

    #[test]
    fn target_uses_its_own_two_references_when_neighbours_have_a_longer_ending() {
        let first = noise(240, 932);
        let second = noise(400, 933);
        let samples = [
            sample(320, &first, 480, 1),
            sample(320, &second, 320, 2),
            sample(320, &second, 320, 3),
            sample(320, &second, 320, 4),
            sample(400, &first, 400, 5),
            sample(480, &first, 320, 6),
        ];
        let (base, start, end) = detect_common(&samples).unwrap();
        assert!(match_template(&samples[0], &samples[base], start, end).is_none());
        let (start, end) = detect_target(&samples).unwrap();
        assert!((start - 1040.0).abs() < 0.25 && (end - 1070.0).abs() < 0.25);
        assert!(
            detect_target(&samples[..5]).is_none(),
            "one reference is insufficient"
        );
        assert!(
            detect_target(&samples[..4]).is_none(),
            "neighbour consensus cannot prove the target"
        );
    }

    #[test]
    fn shortened_references_use_only_the_intersection_starting_later() {
        let common = noise(480, 734);
        let samples = [
            sample(320, &common, 240, 7),
            sample(400, &common, 160, 8),
            sample(320, &common[160..], 400, 9),
        ];
        let (start, end) = detect_target(&samples).unwrap();
        assert!((start - 1060.0).abs() < 0.25, "{start}");
        assert!((end - 1100.0).abs() < 0.25, "{end}");
    }

    #[test]
    fn refuses_invalid_inputs_and_expired_deadlines() {
        let empty = sample(0, &[], 0, 2);
        assert!(detect_common(&[]).is_none());
        assert!(detect_common(&[empty.clone(), empty.clone()]).is_none());
        let fp = sample(80, &noise(200, 91), 80, 2);
        assert!(template_range(&fp, &fp, f64::NAN, 20.0, &Deadline::new(20)).is_none());
        assert!(segment(&fp, &fp, &Deadline(Instant::now() - Duration::from_secs(1))).is_none());
    }
}
