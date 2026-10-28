//! Blu-ray ISO metadata probing using exact HTTP/local byte ranges. No media payload is downloaded.
use crate::{
    AppState,
    error::{ApiError, Result},
    jobs::JobContext,
    state::string,
};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

const SECTOR: u64 = 2048;
const WEB_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
const ALIPAY_UA: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 16_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148 AlipayDefined(nt:WIFI,ws:390|844|3.0) AliApp(AP/10.5.33.8143) AlipayClient/10.5.33.8143 Language/zh-Hans Region/CN";

enum Source {
    Local(PathBuf),
    Http {
        client: Client,
        url: String,
        user_agent: String,
    },
}
pub struct RangeReader {
    source: Source,
    intervals: Vec<(u64, Vec<u8>)>,
    pub requests: usize,
    pub bytes: usize,
    context: Option<JobContext>,
}
impl RangeReader {
    pub fn local(path: impl Into<PathBuf>) -> Self {
        Self::new(Source::Local(path.into()))
    }
    pub fn http(client: Client, url: impl Into<String>, user_agent: impl Into<String>) -> Self {
        Self::new(Source::Http {
            client,
            url: url.into(),
            user_agent: user_agent.into(),
        })
    }
    fn new(source: Source) -> Self {
        Self {
            source,
            intervals: vec![],
            requests: 0,
            bytes: 0,
            context: None,
        }
    }
    pub fn with_context(mut self, context: Option<&JobContext>) -> Self {
        self.context = context.cloned();
        self
    }
    pub async fn read(&mut self, start: u64, length: usize) -> Result<Vec<u8>> {
        if let Some(context) = &self.context {
            context.checkpoint().await?;
        }
        if length == 0 {
            return Ok(vec![]);
        }
        let end = start
            .checked_add(length as u64)
            .ok_or_else(|| ApiError::bad("ISO Range 地址溢出"))?;
        for (cached_start, data) in &self.intervals {
            if start >= *cached_start && end <= *cached_start + data.len() as u64 {
                let offset = (start - *cached_start) as usize;
                return Ok(data[offset..offset + length].to_vec());
            }
        }
        self.requests += 1;
        let content = match &self.source {
            Source::Local(path) => {
                let mut file = tokio::fs::File::open(path).await?;
                file.seek(std::io::SeekFrom::Start(start)).await?;
                let mut data = vec![0; length];
                file.read_exact(&mut data).await.map_err(|_| {
                    ApiError::bad(format!(
                        "本地文件读取失败: expected={length}, offset={start}"
                    ))
                })?;
                data
            }
            Source::Http {
                client,
                url,
                user_agent,
            } => {
                let mut response = client
                    .get(url)
                    .header("Range", format!("bytes={start}-{}", end - 1))
                    .header("User-Agent", user_agent)
                    .header("Accept", "*/*")
                    .header("Accept-Encoding", "identity")
                    .header("Connection", "close")
                    .timeout(Duration::from_secs(60))
                    .send()
                    .await?;
                if response.status() != StatusCode::PARTIAL_CONTENT {
                    return Err(ApiError::bad(format!(
                        "Range 读取失败: HTTP={}, expected={length}",
                        response.status().as_u16()
                    )));
                }
                let mut data = Vec::with_capacity(length);
                while let Some(chunk) = response.chunk().await? {
                    if chunk.len() > length - data.len() {
                        return Err(ApiError::bad("Range 返回数据超过请求长度"));
                    }
                    data.extend_from_slice(&chunk);
                }
                if data.len() != length {
                    return Err(ApiError::bad(format!(
                        "Range 读取失败: got={}, expected={length}",
                        data.len()
                    )));
                }
                data
            }
        };
        self.bytes += content.len();
        self.intervals.push((start, content.clone()));
        Ok(content)
    }
    pub async fn prefetch(
        &mut self,
        ranges: &[(u64, usize)],
        max_gap: u64,
        max_chunk: usize,
    ) -> Result<()> {
        let mut ranges = ranges
            .iter()
            .filter(|(_, n)| *n > 0)
            .map(|(s, n)| {
                s.checked_add(*n as u64)
                    .map(|e| (*s, e))
                    .ok_or_else(|| ApiError::bad("ISO Range 地址溢出"))
            })
            .collect::<Result<Vec<_>>>()?;
        ranges.sort_unstable();
        let mut merged: Vec<(u64, u64)> = vec![];
        for (start, end) in ranges {
            if let Some(last) = merged.last_mut()
                && start <= last.1.saturating_add(max_gap)
                && end.saturating_sub(last.0) <= max_chunk as u64
            {
                last.1 = last.1.max(end);
            } else {
                merged.push((start, end));
            }
        }
        for (start, end) in merged {
            self.read(
                start,
                usize::try_from(end - start).map_err(|_| ApiError::bad("ISO Range 范围过大"))?,
            )
            .await?;
        }
        Ok(())
    }
}
fn slice(data: &[u8], offset: usize, len: usize) -> Result<&[u8]> {
    data.get(
        offset
            ..offset
                .checked_add(len)
                .ok_or_else(|| ApiError::bad("ISO 描述符地址溢出"))?,
    )
    .ok_or_else(|| ApiError::bad("ISO 描述符不完整"))
}
fn le16(data: &[u8], p: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(slice(data, p, 2)?.try_into().unwrap()))
}
fn le32(data: &[u8], p: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(slice(data, p, 4)?.try_into().unwrap()))
}
fn le64(data: &[u8], p: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(slice(data, p, 8)?.try_into().unwrap()))
}
fn be16(data: &[u8], p: usize) -> Option<u16> {
    Some(u16::from_be_bytes(
        data.get(p..p.checked_add(2)?)?.try_into().ok()?,
    ))
}
fn be32(data: &[u8], p: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        data.get(p..p.checked_add(4)?)?.try_into().ok()?,
    ))
}
fn latin(data: &[u8]) -> String {
    data.iter()
        .map(|b| char::from(*b))
        .collect::<String>()
        .trim_matches('\0')
        .into()
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub flags: u8,
    pub length: usize,
    pub lbn: u32,
    pub part: u16,
}
struct Fields {
    allocation: u16,
    info_len: u64,
    start: usize,
    length: usize,
}
struct Allocation {
    length: usize,
    lbn: u32,
    part: Option<u16>,
}
pub struct UdfIsoReader {
    pub reader: RangeReader,
    part_start: u32,
    fsd_len: usize,
    fsd_lbn: u32,
    fsd_part: u16,
    metadata_part: Option<u16>,
    pub meta_blob: Vec<u8>,
}
impl UdfIsoReader {
    pub async fn open(mut reader: RangeReader) -> Result<Self> {
        let avdp = reader.read(256 * SECTOR, SECTOR as usize).await?;
        if le16(&avdp, 0)? != 2 {
            return Err(ApiError::bad("未找到 UDF AVDP"));
        }
        let main_len = le32(&avdp, 16)? as usize;
        let descriptors = reader
            .read(le32(&avdp, 20)? as u64 * SECTOR, main_len)
            .await?;
        let mut part = None;
        let mut volume = None;
        for desc in descriptors.as_chunks::<{ SECTOR as usize }>().0 {
            match le16(desc, 0)? {
                5 => part = Some((le16(desc, 22)?, le32(desc, 188)?)),
                6 => volume = Some(desc),
                8 => break,
                _ => {}
            }
        }
        let ((part_number, part_start), volume) = part
            .zip(volume)
            .ok_or_else(|| ApiError::bad("未找到 UDF 分区信息"))?;
        let fsd_len = (le32(volume, 248)? & 0x3fffffff) as usize;
        let fsd_lbn = le32(volume, 252)?;
        let fsd_part = le16(volume, 256)?;
        let maps = slice(volume, 440, le32(volume, 264)? as usize)?;
        let count = le32(volume, 268)?;
        let mut pos = 0;
        let mut metadata = None;
        let mut physical = None;
        let mut parsed = 0;
        for index in 0..count {
            let Some(header) = maps.get(pos..pos + 2) else {
                break;
            };
            let length = header[1] as usize;
            if length < 2 {
                break;
            }
            let Some(raw) = maps.get(pos..pos + length) else {
                break;
            };
            parsed += 1;
            if header[0] == 1 && length == 6 {
                physical = Some((index, le16(raw, 4)?));
            } else if header[0] == 2
                && length >= 44
                && latin(&raw[5..28]).contains("Metadata Partition")
            {
                metadata = Some((index as u16, le32(raw, 40)?));
                break;
            }
            pos += length;
        }
        if metadata.is_none()
            && !(parsed == 1
                && count == 1
                && physical == Some((fsd_part as u32, part_number))
                && fsd_part == 0)
        {
            return Err(ApiError::bad("未找到 UDF Metadata Partition"));
        }
        let mut iso = Self {
            reader,
            part_start,
            fsd_len,
            fsd_lbn,
            fsd_part,
            metadata_part: metadata.map(|v| v.0),
            meta_blob: vec![],
        };
        if let Some((_, lbn)) = metadata {
            let entry = iso
                .reader
                .read(iso.physical_offset(lbn), SECTOR as usize)
                .await?;
            let fields = Self::fields(&entry)?;
            if fields.allocation == 3 {
                iso.meta_blob = slice(&entry, fields.start, fields.length)?.to_vec();
            } else {
                for ad in Self::allocations(&entry)? {
                    let chunk = iso
                        .reader
                        .read(iso.physical_offset(ad.lbn), ad.length)
                        .await?;
                    iso.meta_blob.extend_from_slice(&chunk);
                }
            }
            if iso.meta_blob.is_empty() {
                return Err(ApiError::bad("UDF Metadata File 没有数据区"));
            }
        }
        Ok(iso)
    }
    fn physical_offset(&self, lbn: u32) -> u64 {
        (self.part_start as u64 + lbn as u64) * SECTOR
    }
    fn fields(data: &[u8]) -> Result<Fields> {
        let tag = le16(data, 0)?;
        let allocation = le16(data, 34)? & 7;
        let (ea, ad, base) = match tag {
            261 => (168, 172, 176),
            266 => (208, 212, 216),
            _ => return Err(ApiError::bad("UDF 文件条目类型不支持")),
        };
        let fields = Fields {
            allocation,
            info_len: le64(data, 56)?,
            start: base + le32(data, ea)? as usize,
            length: le32(data, ad)? as usize,
        };
        slice(data, fields.start, fields.length)?;
        Ok(fields)
    }
    fn allocations(data: &[u8]) -> Result<Vec<Allocation>> {
        let fields = Self::fields(data)?;
        let stride = match fields.allocation {
            0 => 8,
            1 => 16,
            3 => return Ok(vec![]),
            _ => return Err(ApiError::bad("UDF 分配描述符类型不支持")),
        };
        let ads = slice(data, fields.start, fields.length)?;
        let mut result = vec![];
        if ads.len() % stride != 0 {
            return Err(ApiError::bad("UDF 分配描述符不完整"));
        }
        for raw in ads.chunks_exact(stride) {
            let length = le32(raw, 0)?;
            let lbn = le32(raw, 4)?;
            if length >> 30 != 0 {
                return Err(ApiError::bad("UDF 非记录或延续分配描述符不支持"));
            }
            if length > 0 {
                result.push(Allocation {
                    length: (length & 0x3fffffff) as usize,
                    lbn,
                    part: if stride == 16 {
                        Some(le16(raw, 8)?)
                    } else {
                        None
                    },
                });
            }
        }
        Ok(result)
    }
    pub async fn read_partition(&mut self, part: u16, lbn: u32, length: usize) -> Result<Vec<u8>> {
        if part == 0 {
            return self.reader.read(self.physical_offset(lbn), length).await;
        }
        if Some(part) != self.metadata_part {
            return Err(ApiError::bad("UDF 分区引用不支持"));
        }
        slice(&self.meta_blob, lbn as usize * SECTOR as usize, length).map(Vec::from)
    }
    pub async fn read_file_entry(&mut self, entry: &Entry) -> Result<Vec<u8>> {
        self.read_partition(entry.part, entry.lbn, entry.length.max(SECTOR as usize))
            .await
    }
    pub async fn file_data(&mut self, entry: &[u8], part: u16) -> Result<Vec<u8>> {
        let fields = Self::fields(entry)?;
        let mut data = vec![];
        if fields.allocation == 3 {
            return Ok(slice(
                entry,
                fields.start,
                fields.length.min(fields.info_len as usize),
            )?
            .to_vec());
        }
        for ad in Self::allocations(entry)? {
            data.extend_from_slice(
                &self
                    .read_partition(ad.part.unwrap_or(part), ad.lbn, ad.length)
                    .await?,
            );
        }
        if data.len() < fields.info_len as usize {
            return Err(ApiError::bad("UDF 文件数据不完整"));
        }
        data.truncate(fields.info_len as usize);
        Ok(data)
    }
    pub async fn root(&mut self) -> Result<Entry> {
        let fsd = self
            .read_partition(
                self.fsd_part,
                self.fsd_lbn,
                self.fsd_len.max(SECTOR as usize),
            )
            .await?;
        Ok(Entry {
            name: "/".into(),
            flags: 2,
            length: (le32(&fsd, 400)? & 0x3fffffff) as usize,
            lbn: le32(&fsd, 404)?,
            part: le16(&fsd, 408)?,
        })
    }
    pub async fn list_dir(&mut self, entry: &Entry) -> Result<Vec<Entry>> {
        let fe = self.read_file_entry(entry).await?;
        let data = self.file_data(&fe, entry.part).await?;
        let mut pos = 0;
        let mut entries = vec![];
        while pos + 38 <= data.len() {
            let tag = le16(&data, pos)?;
            if tag == 0 {
                break;
            }
            if tag != 257 {
                return Err(ApiError::bad("UDF 目录记录类型不支持"));
            }
            let name_len = data[pos + 19] as usize;
            let impl_len = le16(&data, pos + 36)? as usize;
            let raw = slice(&data, pos + 38 + impl_len, name_len)?;
            let name = match raw.split_first() {
                Some((8, payload)) => latin(payload),
                Some((16, payload)) => String::from_utf16_lossy(
                    &payload
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|c| u16::from_be_bytes([c[0], c[1]]))
                        .collect::<Vec<_>>(),
                ),
                Some((_, payload)) => String::from_utf8_lossy(payload).into_owned(),
                None => String::new(),
            };
            if !name.is_empty() && data[pos + 18] & 0x0c == 0 {
                entries.push(Entry {
                    name,
                    flags: data[pos + 18],
                    length: (le32(&data, pos + 20)? & 0x3fffffff) as usize,
                    lbn: le32(&data, pos + 24)?,
                    part: le16(&data, pos + 28)?,
                });
            }
            pos += (38 + impl_len + name_len + 3) & !3;
        }
        Ok(entries)
    }
    async fn find(&mut self, entry: &Entry, name: &str) -> Result<Option<Entry>> {
        Ok(self
            .list_dir(entry)
            .await?
            .into_iter()
            .find(|e| e.name.eq_ignore_ascii_case(name)))
    }
    async fn prefetch_files(
        &mut self,
        entries: &[Entry],
        max_gap: u64,
        max_chunk: usize,
    ) -> Result<()> {
        let mut ranges = vec![];
        for entry in entries {
            let fe = self.read_file_entry(entry).await?;
            for ad in Self::allocations(&fe)? {
                if ad.part.unwrap_or(entry.part) == 0 {
                    ranges.push((self.physical_offset(ad.lbn), ad.length));
                }
            }
        }
        self.reader.prefetch(&ranges, max_gap, max_chunk).await
    }
    async fn info_len(&mut self, entry: &Entry) -> Result<u64> {
        Ok(Self::fields(&self.read_file_entry(entry).await?)?.info_len)
    }
}

fn mpls_stream(data: &[u8], pos: usize, end: usize) -> Option<(Value, usize)> {
    let length = *data.get(pos)? as usize;
    let start = pos.checked_add(1)?;
    let descriptor_end = start.checked_add(length)?;
    if length < 1 || descriptor_end > end {
        return None;
    }
    let stream_type = data[start];
    let pid = match stream_type {
        1 if length >= 3 => be16(data, start + 1)?,
        2 if length >= 5 => be16(data, start + 3)?,
        3 | 4 if length >= 4 => be16(data, start + 2)?,
        _ => return None,
    };
    let length = *data.get(descriptor_end)? as usize;
    let start = descriptor_end + 1;
    let attribute_end = start.checked_add(length)?;
    if length < 1 || attribute_end > end {
        return None;
    }
    let coding = data[start];
    let mut rate = 0;
    let mut language = String::new();
    match coding {
        0x01 | 0x02 | 0xea | 0x1b | 0x24 if length >= 2 => rate = data[start + 1],
        0x03 | 0x04 | 0x80 | 0x81 | 0x82 | 0x83 | 0x84 | 0x85 | 0x86 | 0xa1 | 0xa2
            if length >= 5 =>
        {
            rate = data[start + 1];
            language = latin(&data[start + 2..start + 5]);
        }
        0x90 | 0x91 if length >= 4 => language = latin(&data[start + 1..start + 4]),
        0x92 if length >= 5 => language = latin(&data[start + 2..start + 5]),
        _ => {}
    }
    Some((
        json!({"stream_type":stream_type,"pid":pid,"coding_type":coding,"format_rate":rate,"language":language}),
        attribute_end,
    ))
}
fn mpls_stn(data: &[u8], pos: usize, end: usize) -> Option<Value> {
    if pos.checked_add(16)? > end {
        return None;
    }
    let length = be16(data, pos)? as usize;
    let start = pos + 2;
    let stop = start.checked_add(length)?;
    if length < 14 || stop > end {
        return None;
    }
    let mut current = start + 14;
    let mut groups = json!({});
    for (i, name) in ["video", "audio", "subtitle"].iter().enumerate() {
        let mut entries = vec![];
        for _ in 0..data[start + 2 + i] {
            let (entry, next) = mpls_stream(data, current, stop)?;
            entries.push(entry);
            current = next;
        }
        groups[name] = json!(entries);
    }
    Some(groups)
}
pub fn parse_mpls(data: &[u8], sizes: &BTreeMap<String, u64>) -> Option<Value> {
    if !data.starts_with(b"MPLS") || data.len() < 64 {
        return None;
    }
    let start = be32(data, 8)? as usize;
    if start == 0 || start.checked_add(10)? > data.len() {
        return None;
    }
    let count = be16(data, start + 6)?;
    let mut pos = start + 10;
    let mut clips = vec![];
    let mut duration = 0.0;
    let mut complete = true;
    for _ in 0..count {
        let Some(length) = be16(data, pos).map(usize::from) else {
            complete = false;
            break;
        };
        let Some(end) = pos.checked_add(2 + length).filter(|e| *e <= data.len()) else {
            complete = false;
            break;
        };
        if length < 20 {
            complete = false;
            break;
        }
        let clip = format!(
            "{}.m2ts",
            String::from_utf8_lossy(data.get(pos + 2..pos + 7)?)
        );
        let seconds = be32(data, pos + 18)?.saturating_sub(be32(data, pos + 14)?) as f64 / 45000.0;
        duration += seconds;
        let mut stn = None;
        let mut stn_pos = pos + 34;
        if length >= 32 && stn_pos <= end {
            if be16(data, pos + 11)? & 0x10 != 0 {
                if stn_pos + 2 <= end {
                    let angles = data[stn_pos].max(1) as usize;
                    stn_pos += 2 + (angles - 1) * 10;
                } else {
                    stn_pos = end + 1;
                }
            }
            if stn_pos <= end {
                stn = mpls_stn(data, stn_pos, end);
            }
        }
        clips.push(json!({"clip":clip,"seconds":seconds,"stn_complete":stn.is_some(),"stn":stn}));
        pos = end;
    }
    if clips.is_empty() {
        return None;
    }
    let unique = clips
        .iter()
        .filter_map(|v| v["clip"].as_str())
        .collect::<BTreeSet<_>>();
    Some(
        json!({"items":clips.len(),"duration":duration,"unique_clip_count":unique.len(),"unique_size":unique.iter().map(|n|sizes.get(*n).copied().unwrap_or(0)).sum::<u64>(),"max_clip_size":unique.iter().map(|n|sizes.get(*n).copied().unwrap_or(0)).max().unwrap_or(0),"stn_complete":complete&&clips.len()==count as usize&&clips.iter().all(|v|v["stn_complete"]==true),"clips":clips}),
    )
}
fn array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn rank(playlist: &Value) -> (u64, u64) {
    (
        playlist["max_clip_size"].as_u64().unwrap_or(0),
        playlist["unique_size"].as_u64().unwrap_or(0),
    )
}
pub fn select_main_playlist(playlists: &[Value]) -> Result<(Value, Value)> {
    let mut distinct = BTreeMap::<Vec<String>, &Value>::new();
    for p in playlists {
        let key = array(&p["clips"])
            .iter()
            .map(|v| string(v, "clip").to_lowercase())
            .collect();
        if distinct
            .get(&key)
            .is_none_or(|old| (rank(p), string(p, "name")) > (rank(old), string(old, "name")))
        {
            distinct.insert(key, p);
        }
    }
    let mut candidates = distinct.into_values().collect::<Vec<_>>();
    candidates.sort_by_key(|p| std::cmp::Reverse((rank(p), string(p, "name"))));
    let selected = candidates
        .first()
        .ok_or_else(|| ApiError::bad("未解析到可用 MPLS"))?;
    let equal = candidates
        .iter()
        .skip(1)
        .filter(|p| rank(p) == rank(selected))
        .collect::<Vec<_>>();
    let reason = if equal.is_empty() {
        String::new()
    } else {
        format!(
            "存在物理规模相同的主播放列表: {}",
            equal
                .iter()
                .take(3)
                .map(|p| string(p, "name"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    Ok((
        (*selected).clone(),
        json!({"ambiguous":!equal.is_empty(),"reason":reason,"distinct_count":candidates.len(),"duplicate_count":playlists.len()-candidates.len()}),
    ))
}
fn codec(coding: u8) -> (&'static str, String, &'static str) {
    let (kind, name, profile) = match coding {
        0x01 => ("video", "mpeg1video", ""),
        0x02 => ("video", "mpeg2video", ""),
        0x1b => ("video", "h264", ""),
        0xea => ("video", "vc1", ""),
        0x24 => ("video", "hevc", ""),
        0x80 => ("audio", "pcm_bluray", ""),
        0x81 => ("audio", "ac3", ""),
        0x82 => ("audio", "dts", ""),
        0x83 => ("audio", "truehd", "Dolby TrueHD"),
        0x84 => ("audio", "eac3", ""),
        0x85 => ("audio", "dts", "DTS-HD HRA"),
        0x86 => ("audio", "dts", "DTS-HD MA"),
        0xa1 => ("audio", "eac3", "Secondary Audio"),
        0xa2 => ("audio", "dts", "Secondary Audio"),
        0x90 | 0x91 => ("subtitle", "hdmv_pgs_subtitle", ""),
        0x92 => ("subtitle", "text", ""),
        _ => ("other", "", ""),
    };
    (
        kind,
        if name.is_empty() {
            format!("0x{coding:02x}")
        } else {
            name.into()
        },
        profile,
    )
}
fn name_effect(name: &str, token: &str) -> bool {
    regex::Regex::new(&format!(r"(?i)(^|[ ._\-])({token})([ ._\-]|$)"))
        .unwrap()
        .is_match(name)
}
fn channels(rate: u8, codec: &str, name: &str) -> u8 {
    if ["truehd", "dts"].contains(&codec)
        && regex::Regex::new(r"(?:^|[^0-9])7[._ ]?1(?:[^0-9]|$)")
            .unwrap()
            .is_match(name)
    {
        return 8;
    }
    if regex::Regex::new(r"(?:^|[^0-9])5[._ ]?1(?:[^0-9]|$)")
        .unwrap()
        .is_match(name)
    {
        return 6;
    }
    match rate >> 4 {
        1 => 1,
        3 => 2,
        6 => 6,
        12 => 8,
        _ => 0,
    }
}
fn dv() -> Value {
    json!([{"side_data_type":"DOVI configuration record","dv_profile":7,"dv_bl_signal_compatibility_id":6}])
}
pub fn parse_clpi_streams(data: &[u8], name: &str) -> Vec<Value> {
    if !data.starts_with(b"HDMV") || data.len() < 64 {
        return vec![];
    }
    let Some(start) = be32(data, 12).map(|v| v as usize) else {
        return vec![];
    };
    if start == 0 || start.saturating_add(8) > data.len() {
        return vec![];
    }
    let Some(length) = be32(data, start).map(|v| v as usize) else {
        return vec![];
    };
    let end = data
        .len()
        .min(start.saturating_add(4).saturating_add(length));
    let score = |items: &[Value]| items.iter().filter(|v| v["codec_type"] != "other").count();
    let mut best = vec![];
    for delta in 10..20 {
        for payload in [true, false] {
            let mut pos = start + delta;
            let mut items = vec![];
            while pos.saturating_add(3) <= end && items.len() < 96 {
                let Some(pid) = be16(data, pos) else {
                    break;
                };
                let length = data[pos + 2] as usize;
                if length < 4 {
                    break;
                }
                let next = pos.saturating_add(if payload { 3 + length } else { length });
                let Some(raw) = data.get(pos + 3..next) else {
                    break;
                };
                if raw.is_empty() {
                    break;
                }
                let (kind, codec, profile) = codec(raw[0]);
                let mut item = json!({"index":items.len(),"id":pid,"codec_type":kind,"codec_name":codec,"profile":profile,"tags":{},"disposition":{}});
                if kind == "video" && raw.len() >= 4 {
                    let (width, height) = match raw[1] >> 4 {
                        1 | 3 => (720, 480),
                        2 | 7 => (720, 576),
                        4 | 6 => (1920, 1080),
                        5 => (1280, 720),
                        8 => (3840, 2160),
                        _ => (0, 0),
                    };
                    let fps = match raw[1] & 15 {
                        1 => "24000/1001",
                        2 => "24/1",
                        3 => "25/1",
                        4 => "30000/1001",
                        6 => "50/1",
                        7 => "60000/1001",
                        _ => "0/0",
                    };
                    item.as_object_mut().unwrap().extend(json!({"width":width,"height":height,"avg_frame_rate":fps,"r_frame_rate":fps,"display_aspect_ratio":if raw[2]>>4==3{"16:9"}else{""}}).as_object().unwrap().clone());
                    if codec == "hevc" {
                        item["pix_fmt"] = json!("yuv420p10le");
                        item["bits_per_raw_sample"] = json!("10");
                    }
                    if width >= 3800 {
                        item["color_space"] = json!("bt2020nc");
                        item["color_primaries"] = json!("bt2020");
                        item["color_transfer"] = json!("smpte2084");
                    }
                    if name_effect(name, r"DV|DOVI|DOLBY[ ._\-]*VISION") {
                        item["side_data_list"] = dv();
                    }
                } else if kind == "audio" && raw.len() >= 5 {
                    let count = channels(raw[1], &codec, name);
                    item["tags"]["language"] = json!(latin(&raw[2..5]));
                    item["channels"] = json!(count);
                    item["sample_rate"] = json!(match raw[1] & 15 {
                        1 => "48000",
                        4 => "96000",
                        5 => "192000",
                        _ => "",
                    });
                    if count > 0 {
                        item["channel_layout"] = json!(match count {
                            8 => "7.1",
                            6 => "5.1",
                            2 => "stereo",
                            _ => "mono",
                        });
                    }
                } else if kind == "subtitle" && raw.len() >= 4 {
                    item["tags"]["language"] = json!(latin(&raw[1..4]));
                }
                items.push(item);
                pos = next;
            }
            if score(&items) > score(&best) {
                best = items;
            }
        }
    }
    best.into_iter()
        .filter(|v| v["codec_type"] != "other")
        .collect()
}
pub fn filter_clpi_streams_by_stn(streams: &[Value], stn: &Value) -> (Vec<Value>, bool, String) {
    if !stn.is_object() {
        return (
            streams.to_vec(),
            false,
            "MPLS PlayItem 缺少 STN 主轨道表".into(),
        );
    }
    let mut filtered = vec![];
    let mut missing = vec![];
    let mut keys = BTreeSet::new();
    for kind in ["video", "audio", "subtitle"] {
        for entry in array(&stn[kind]) {
            let pid = entry["pid"].as_u64().unwrap_or(0);
            if !keys.insert((kind, pid)) {
                missing.push(format!("{kind} PID 0x{pid:04x} 重复"));
                continue;
            }
            let matches = streams
                .iter()
                .filter(|v| v["codec_type"] == kind && v["id"].as_u64() == Some(pid))
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                missing.push(format!("{kind} PID 0x{pid:04x}"));
            } else {
                filtered.push(matches[0].clone());
            }
        }
    }
    if !missing.is_empty() {
        (
            streams.to_vec(),
            false,
            format!(
                "CLPI 未唯一解析 {}",
                missing.into_iter().take(4).collect::<Vec<_>>().join(", ")
            ),
        )
    } else {
        (filtered, true, String::new())
    }
}
pub fn collect_playlist_stn(playlist: &Value) -> (BTreeMap<String, Value>, Vec<String>) {
    let mut occurrences = BTreeMap::<String, Vec<&Value>>::new();
    for item in array(&playlist["clips"]) {
        let clip = string(item, "clip");
        if !clip.is_empty() {
            occurrences.entry(clip).or_default().push(item);
        }
    }
    let mut result = BTreeMap::new();
    let mut failures = vec![];
    for (clip, items) in occurrences {
        if items
            .iter()
            .any(|item| item["stn_complete"] != true || !item["stn"].is_object())
        {
            failures.push(format!("{clip} 缺少 STN"));
            continue;
        }
        let signatures = items
            .iter()
            .map(|item| {
                json!(
                    ["video", "audio", "subtitle"]
                        .iter()
                        .map(|kind| json!([
                            kind,
                            array(&item["stn"][kind])
                                .iter()
                                .map(|e| json!([
                                    e["pid"],
                                    e["coding_type"],
                                    string(e, "language").to_lowercase()
                                ]))
                                .collect::<Vec<_>>()
                        ]))
                        .collect::<Vec<_>>()
                )
                .to_string()
            })
            .collect::<BTreeSet<_>>();
        if signatures.len() != 1 {
            failures.push(format!("{clip} 的多个 PlayItem STN 不一致"));
        } else {
            result.insert(clip, items[0]["stn"].clone());
        }
    }
    (result, failures)
}
fn semantic_signature(streams: &[Value]) -> String {
    json!(
        streams
            .iter()
            .filter(|v| ["video", "audio", "subtitle"].contains(&string(v, "codec_type").as_str()))
            .map(|v| json!([
                string(v, "codec_type").to_lowercase(),
                string(v, "codec_name").to_lowercase(),
                string(v, "profile").to_lowercase(),
                v["channels"].as_u64().unwrap_or(0),
                string(v, "sample_rate"),
                v["width"].as_u64().unwrap_or(0),
                v["height"].as_u64().unwrap_or(0),
                string(&v["tags"], "language").to_lowercase()
            ]))
            .collect::<Vec<_>>()
    )
    .to_string()
}
pub fn raw_clpi_layout_status(
    clips: &[String],
    streams: &BTreeMap<String, Vec<Value>>,
) -> (usize, bool, String) {
    let missing = clips
        .iter()
        .filter(|c| !streams.contains_key(*c))
        .collect::<Vec<_>>();
    let count = streams
        .values()
        .map(|v| semantic_signature(v))
        .collect::<BTreeSet<_>>()
        .len();
    let reason = if !missing.is_empty() {
        format!(
            "主播放列表缺少 CLPI: {}",
            missing
                .into_iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        )
    } else if count != 1 {
        "主播放列表内多个片段的原始媒体流布局不一致".into()
    } else {
        String::new()
    };
    (count, !reason.is_empty(), reason)
}
pub async fn probe_reader(
    reader: RangeReader,
    name: &str,
    size: u64,
    url_method: &str,
) -> Result<Value> {
    let started = Instant::now();
    let mut iso = UdfIsoReader::open(reader).await?;
    let root = iso.root().await?;
    let bdmv = iso
        .find(&root, "BDMV")
        .await?
        .ok_or_else(|| ApiError::bad("ISO 内没有 BDMV 目录"))?;
    let playlist_dir = iso.find(&bdmv, "PLAYLIST").await?;
    let stream_dir = iso.find(&bdmv, "STREAM").await?;
    let clipinf_dir = iso.find(&bdmv, "CLIPINF").await?;
    let (Some(playlist_dir), Some(stream_dir), Some(clipinf_dir)) =
        (playlist_dir, stream_dir, clipinf_dir)
    else {
        return Err(ApiError::bad("BDMV 目录缺少 PLAYLIST/STREAM/CLIPINF"));
    };
    let mut sizes = BTreeMap::new();
    for entry in iso.list_dir(&stream_dir).await? {
        sizes.insert(entry.name.clone(), iso.info_len(&entry).await?);
    }
    if sizes.is_empty() {
        return Err(ApiError::bad("BDMV STREAM 目录为空"));
    }
    let entries = iso.list_dir(&playlist_dir).await?;
    iso.prefetch_files(&entries, 262144, 16 * 1024 * 1024)
        .await?;
    let mut playlists = vec![];
    for entry in entries {
        let fe = iso.read_file_entry(&entry).await?;
        let data = iso.file_data(&fe, entry.part).await?;
        if let Some(mut parsed) = parse_mpls(&data, &sizes) {
            parsed["name"] = json!(entry.name);
            playlists.push(parsed);
        }
    }
    let (main, selection) = select_main_playlist(&playlists)?;
    let mut clips = vec![];
    for clip in array(&main["clips"]) {
        let name = string(clip, "clip");
        if !clips.contains(&name) {
            clips.push(name);
        }
    }
    let main_clip = clips
        .iter()
        .max_by_key(|n| (sizes.get(*n).copied().unwrap_or(0), (*n).clone()))
        .ok_or_else(|| ApiError::bad("主播放列表没有片段"))?
        .clone();
    let entries = iso
        .list_dir(&clipinf_dir)
        .await?
        .into_iter()
        .map(|e| (e.name.to_lowercase(), e))
        .collect::<BTreeMap<_, _>>();
    let selected = clips
        .iter()
        .filter_map(|c| {
            entries
                .get(&format!("{}.clpi", c.trim_end_matches(".m2ts")).to_lowercase())
                .map(|e| (c.clone(), e.clone()))
        })
        .collect::<Vec<_>>();
    iso.prefetch_files(
        &selected.iter().map(|(_, e)| e.clone()).collect::<Vec<_>>(),
        0,
        1024 * 1024,
    )
    .await?;
    let mut by_clip = BTreeMap::new();
    for (clip, entry) in selected {
        let fe = iso.read_file_entry(&entry).await?;
        let streams = parse_clpi_streams(&iso.file_data(&fe, entry.part).await?, name);
        if !streams.is_empty() {
            by_clip.insert(clip, streams);
        }
    }
    let mut streams = by_clip
        .get(&main_clip)
        .filter(|s| !s.is_empty())
        .cloned()
        .ok_or_else(|| ApiError::bad(format!("未解析到主片流信息: {main_clip}")))?;
    let (layout_count, ambiguous, reason) = raw_clpi_layout_status(&clips, &by_clip);
    for stream in &mut streams {
        if stream["codec_type"] == "video" {
            if name_effect(name, r"HDR10\+|HDR10|HDR")
                || stream["height"].as_u64().unwrap_or(0) >= 2160
            {
                for (key, value) in [
                    ("color_space", "bt2020nc"),
                    ("color_primaries", "bt2020"),
                    ("color_transfer", "smpte2084"),
                ] {
                    if stream.get(key).is_none() {
                        stream[key] = json!(value);
                    }
                }
            }
            if name_effect(name, r"DV|DOVI|DOLBY[ ._\-]*VISION")
                && stream.get("side_data_list").is_none()
            {
                stream["side_data_list"] = dv();
            }
        }
    }
    let duration = main["duration"].as_f64().unwrap_or(0.0);
    let clip_size = sizes.get(&main_clip).copied().unwrap_or(0);
    let mut raw = json!({"format":{"filename":name,"format_name":"bluray,iso","format_long_name":"Blu-ray ISO","duration":if duration>0.0{format!("{duration:.3}")}else{String::new()},"size":if size>0{size}else{clip_size}.to_string()},"streams":streams,"chapters":[],"_iso_probe":{"main_playlist":main["name"],"main_clip":main_clip,"main_clip_size":clip_size,"playlist_count":playlists.len(),"distinct_playlist_count":selection["distinct_count"],"duplicate_playlist_count":selection["duplicate_count"],"playlist_ambiguous":selection["ambiguous"],"playlist_ambiguity_reason":selection["reason"],"playlist_clip_count":clips.len(),"clip_layout_count":layout_count,"clip_layout_ambiguous":ambiguous,"playlist_pid_filtered":false,"probe_scope":"playlist_clpi_raw","raw_stream_count":streams.len(),"selected_stream_count":streams.len(),"video_stream_count":streams.iter().filter(|s|s["codec_type"]=="video").count(),"reference_safe":!ambiguous,"reference_ambiguity_reason":reason,"range_requests":iso.reader.requests,"range_bytes":iso.reader.bytes,"url_method":url_method,"elapsed_sec":(started.elapsed().as_secs_f64()*1000.0).round()/1000.0}});
    if size > 0 && duration > 0.0 {
        raw["format"]["bit_rate"] = json!(((size as f64 * 8.0 / duration) as u64).to_string());
    }
    Ok(raw)
}
pub async fn probe_file(path: &Path, context: Option<&JobContext>) -> Result<Value> {
    let path = tokio::fs::canonicalize(path).await?;
    let size = tokio::fs::metadata(&path).await?.len();
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    probe_reader(
        RangeReader::local(path).with_context(context),
        &name,
        size,
        "cd2-local",
    )
    .await
}
pub async fn probe_115(
    state: &AppState,
    pc: &str,
    name: &str,
    size: u64,
    context: Option<&JobContext>,
) -> Result<Value> {
    let _slot = crate::strm::slot(state, "main", context).await?;
    let mut last = None;
    for openapi in [false, true] {
        for ua in [WEB_UA, ALIPAY_UA] {
            if let Some(context) = context {
                context.checkpoint().await?;
            }
            let result = if openapi {
                crate::p115::openapi_downurl_with_user_agent(state, pc, ua).await
            } else {
                crate::p115::download_url_with_user_agent(state, pc, ua).await
            };
            match result {
                Ok(url) => {
                    return probe_reader(
                        RangeReader::http(state.http.clone(), url, ua).with_context(context),
                        name,
                        size,
                        if openapi {
                            "openapi_downurl"
                        } else {
                            "download_url"
                        },
                    )
                    .await;
                }
                Err(error) => {
                    if context.is_some_and(JobContext::is_cancelled) {
                        return Err(error);
                    }
                    last = Some(error);
                }
            }
        }
    }
    Err(last.unwrap_or_else(|| ApiError::bad("获取 115 ISO 直链失败")))
}
