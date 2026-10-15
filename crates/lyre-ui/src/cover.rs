//! 封面缓存：按封面内容哈希 + 缩放档位去重，解码时在软件里缩到界面要用的尺寸。
//! 必须自己缩：gpui 的 Windows 渲染器按原始尺寸上传图集，纹理固定 `MipLevels: 1` 且不生成 mip 链。
//! LRU 淘汰会连带清掉指向它的槽位，观察方发现图没了就重新请求。

use std::collections::{HashMap, VecDeque, hash_map::Entry};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use lyre_core::TrackId;

use crate::platform::*;

/// 缩放档位，即位图长边的像素数。相邻两档差 1.33 倍，任一档落到槽位后 GPU 最多再缩 2.3 倍。
const BUCKETS: [u32; 7] = [64, 96, 128, 192, 256, 384, 512];

/// 最高档位，也是「没匹配到任何档」时的兜底。
const MAX_BUCKET: u32 = 512;
const _: () = assert!(MAX_BUCKET == BUCKETS[BUCKETS.len() - 1]);

/// 界面槽位尺寸（逻辑像素）→ 该取哪一档。
/// 乘 1.5 是给 Windows 常见的 1.0–1.25 倍 DPI 留余量：窗口缩放因子在 `&mut App` 上拿不到。
pub fn bucket_for(size: f32) -> u32 {
    let need = (size * 1.5).ceil().max(1.0) as u32;
    *BUCKETS
        .iter()
        .find(|bucket| **bucket >= need)
        .unwrap_or(&MAX_BUCKET)
}

/// 常驻位图的字节预算，不是张数：最高一档 512×512 BGRA 是 1MB，最低一档 64×64 只有 16KB。
/// 32MB 按本仓库曲库的量级定：约百张封面、每档各一张，全装得下还有余量。
const MAX_BYTES: usize = 32 * 1024 * 1024;

/// 同时在解码的封面数上限。解码要按原尺寸分配一次 RGBA（库里最大的一张是 1366×1366 ≈ 7.5MB）。
/// 等名额用定时重试而不是阻塞线程：等待者占着线程会把正在解的饿死。
const MAX_CONCURRENT_DECODES: usize = 4;
const DECODE_SLOT_POLL: Duration = Duration::from_millis(20);

/// 一张位图的身份：封面内容哈希 + 档位。同一张封面的不同档位要各自缓存、各自淘汰。
type ImageId = (u64, u32);

/// 缓存里的一条位图，连同它占的字节数 —— 字节数在解码时算好存着，淘汰时不必回头问 gpui 要尺寸。
struct Cached {
    image: Arc<RenderImage>,
    bytes: usize,
}

enum Slot {
    Loading,
    Ready(ImageId),
    /// 确认没有内嵌封面，或解码失败
    Missing,
}

/// 缓存槽位的键：曲目内嵌封面按 `TrackId`，歌单封面按图片路径的哈希（借 `TrackId::from_path`）。
/// 两者必须是不同变体，否则给歌单设封面可能命中某首曲目的图。档位也是键的一部分。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum CoverKey {
    Track(TrackId, u32),
    File(TrackId, u32),
}

pub struct CoverCache {
    slots: HashMap<CoverKey, Slot>,
    images: HashMap<ImageId, Cached>,
    /// 每张去重后的图被哪些槽位引用，淘汰时照它清槽。
    holders: HashMap<ImageId, Vec<CoverKey>>,
    /// LRU 顺序（按位图身份），队首最旧
    order: VecDeque<ImageId>,
    /// `images` 里所有位图的字节数之和，与它同步增减。淘汰判断看这个数。
    bytes: usize,
    /// 字节预算，默认 [`MAX_BYTES`]。
    limit: usize,
}

impl Default for CoverCache {
    fn default() -> Self {
        Self {
            slots: HashMap::new(),
            images: HashMap::new(),
            holders: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            limit: MAX_BYTES,
        }
    }
}

impl CoverCache {
    /// 歌单等独立图片文件是否已就绪。`size` 是界面槽位的逻辑像素边长。
    pub fn ready_file(&self, path: &Path, size: f32) -> Option<Arc<RenderImage>> {
        self.slot(CoverKey::File(TrackId::from_path(path), bucket_for(size)))
    }

    fn slot(&self, key: CoverKey) -> Option<Arc<RenderImage>> {
        match self.slots.get(&key)? {
            Slot::Ready(image) => self
                .images
                .get(image)
                .map(|cached| Arc::clone(&cached.image)),
            _ => None,
        }
    }

    /// 按需加载一个图片文件。
    pub fn request_file(&mut self, path: PathBuf, size: f32, cx: &mut Context<Self>) {
        let key = CoverKey::File(TrackId::from_path(&path), bucket_for(size));
        if self.slots.contains_key(&key) {
            return;
        }
        self.slots.insert(key, Slot::Loading);
        cx.notify();

        let bucket = bucket_of(&key);
        Self::decode_later(key, move || load_file(&path, bucket), cx);
    }

    /// 已解码好的封面；无副作用、不触发重绘，取不到时返回 `None` 由调用方退化成占位图。
    pub fn ready(&self, id: TrackId, size: f32) -> Option<Arc<RenderImage>> {
        self.slot(CoverKey::Track(id, bucket_for(size)))
    }

    /// 这张封面是否已经被确认「没有」—— 用来区分「还在加载」和「确实没有封面」。
    pub fn is_missing(&self, id: TrackId, size: f32) -> bool {
        matches!(
            self.slots.get(&CoverKey::Track(id, bucket_for(size))),
            Some(Slot::Missing)
        )
    }

    /// 按需加载。已缓存或已在加载中时直接返回，可反复调用。
    pub fn request(&mut self, id: TrackId, path: PathBuf, size: f32, cx: &mut Context<Self>) {
        let key = CoverKey::Track(id, bucket_for(size));
        if self.slots.contains_key(&key) {
            return;
        }
        self.slots.insert(key, Slot::Loading);
        cx.notify();

        let bucket = bucket_of(&key);
        Self::decode_later(key, move || load(&path, bucket), cx);
    }

    /// 后台解码：先排一个解码名额，再丢进线程池。
    fn decode_later(
        key: CoverKey,
        decode: impl FnOnce() -> Option<Decoded> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        // App::background_executor() 返回引用，必须 clone 出所有权值才能带进 'static 任务
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let slot = DecodeSlot::acquire(&executor).await;
            let decoded = executor.spawn(async move { decode() }).await;
            drop(slot);
            this.update(cx, |cache, cx| {
                // 淘汰要一路做到显存：gpui 按 `ImageId` 把上传过的图钉在图集里，
                // 不调 `drop_image` 就不释放
                for evicted in cache.store_key(key, decoded) {
                    cx.drop_image(evicted, None);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 存一张解码产物，返回被淘汰的位图供调用方归还显存 —— `App::drop_image` 要 `&mut App`。
    fn store_key(&mut self, key: CoverKey, decoded: Option<Decoded>) -> Vec<Arc<RenderImage>> {
        let Some((image, frame, bytes)) = decoded else {
            self.slots.insert(key, Slot::Missing);
            return Vec::new();
        };

        // 同一张封面的多个槽位共用一份位图：第二次遇到只补引用，不重新上传也不重复计字节。
        match self.images.entry(image) {
            Entry::Vacant(empty) => {
                empty.insert(Cached {
                    image: Arc::new(RenderImage::new([frame])),
                    bytes,
                });
                self.bytes += bytes;
            }
            Entry::Occupied(_) => {}
        }
        self.touch(image);
        // 这一格改指别的图了，就从旧图的引用里摘掉，否则淘汰旧图会连累这一格。
        if let Some(Slot::Ready(previous)) = self.slots.insert(key, Slot::Ready(image))
            && previous != image
            && let Some(held) = self.holders.get_mut(&previous)
        {
            held.retain(|item| *item != key);
        }
        let held = self.holders.entry(image).or_default();
        if !held.contains(&key) {
            held.push(key);
        }
        self.evict()
    }

    fn touch(&mut self, image: ImageId) {
        if let Some(position) = self.order.iter().position(|h| *h == image) {
            self.order.remove(position);
        }
        self.order.push_back(image);
    }

    /// 超预算就从最旧的开始淘汰，返回被淘汰的位图供调用方归还显存。
    /// 至少留一张：全清空的话下一帧同一格又会重新请求，变成「解码—淘汰」的死循环。
    fn evict(&mut self) -> Vec<Arc<RenderImage>> {
        let mut evicted = Vec::new();
        while self.bytes > self.limit && self.images.len() > 1 {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            evicted.extend(self.drop_image(oldest));
        }
        evicted
    }

    /// 丢一张位图，连带清掉指向它的槽位 —— 否则留下「说是 Ready 却取不到图」的悬空引用。
    fn drop_image(&mut self, image: ImageId) -> Option<Arc<RenderImage>> {
        let cached = self.images.remove(&image)?;
        self.bytes -= cached.bytes;
        if let Some(held) = self.holders.remove(&image) {
            for key in held {
                self.slots.remove(&key);
            }
        }
        Some(cached.image)
    }
}

/// 一个解码名额。`Drop` 归还，所以解码中途 panic 也不会把名额漏掉。
struct DecodeSlot;

/// 正在解码的封面数。进程级：后台池是共享的，几个 `CoverCache` 各限各的等于没限。
static DECODING: AtomicUsize = AtomicUsize::new(0);

impl DecodeSlot {
    async fn acquire(executor: &BackgroundExecutor) -> Self {
        loop {
            // 读—判—写必须一步完成，否则两个任务同时看到「还差一个」就会一起过界
            let current = DECODING.load(Ordering::Relaxed);
            if current < MAX_CONCURRENT_DECODES
                && DECODING
                    .compare_exchange(current, current + 1, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
            {
                return Self;
            }
            executor.timer(DECODE_SLOT_POLL).await;
        }
    }
}

impl Drop for DecodeSlot {
    fn drop(&mut self) {
        DECODING.fetch_sub(1, Ordering::Release);
    }
}

/// 从槽位键里取回它的档位。请求路径上键和后台任务的缩放目标必须来自同一处，
/// 否则缓存里会存进一张尺寸对不上的图。
fn bucket_of(key: &CoverKey) -> u32 {
    match key {
        CoverKey::Track(_, bucket) | CoverKey::File(_, bucket) => *bucket,
    }
}

/// 后台线程的产物：位图身份 + 已缩放的位图 + 它占的字节数。
/// 字节数在这里一起带回来，因为 `RenderImage` 拿到 `frame` 之后就问不到尺寸了。
type Decoded = (ImageId, image::Frame, usize);

/// 读内嵌封面、EXIF 转正、降采样、转成 BGRA；整段在后台线程上跑。
/// `RenderImage` 要 BGRA，而 gpui 对 `ImageSource::Render` 完全信任调用方、不做通道转换。
fn load(path: &Path, bucket: u32) -> Option<Decoded> {
    let cover = lyre_core::read_cover(path).ok().flatten()?;

    let hash = fnv1a(&cover.data);

    let (frame, bytes) = decode_bgra(&cover.data, bucket)?;
    Some(((hash, bucket), frame, bytes))
}

/// 读一个独立图片文件（歌单封面），字节来自磁盘而不是标签。
fn load_file(path: &Path, bucket: u32) -> Option<Decoded> {
    let bytes = std::fs::read(path).ok()?;
    let hash = fnv1a(&bytes);
    let (frame, bytes) = decode_bgra(&bytes, bucket)?;
    Some(((hash, bucket), frame, bytes))
}

/// 封面字节 → gpui 要的 BGRA 帧，长边缩到 `bucket`。EXIF 方向在这里转正。
/// 第二个返回值是这张位图的字节数，缓存预算按它算。
fn decode_bgra(bytes: &[u8], bucket: u32) -> Option<(image::Frame, usize)> {
    use image::ImageDecoder as _;

    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = decoder.orientation().ok()?;
    let mut full = image::DynamicImage::from_decoder(decoder).ok()?;

    // 顺序对齐 gpui：先按 EXIF 转正，再缩到这一档的尺寸
    full.apply_orientation(orientation);
    // `thumbnail` 是面积型重采样，不会放大：小于这一档的图原样返回。
    let thumbnail = full.thumbnail(bucket, bucket);

    let mut bgra = thumbnail.into_rgba8();
    for pixel in bgra.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    let bytes = bgra.as_raw().len();
    Some((image::Frame::new(bgra), bytes))
}

/// FNV-1a。只用来做缓存键，不需要抗碰撞强度，但需要跨进程稳定。
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    // 不能写 `use super::*`：glob 会把 gpui 导出的同名 `test` 过程宏带进来，
    // 遮蔽内置 `#[test]` 并报 `recursion limit reached while expanding #[test]`。
    #[test]
    fn fnv1a_is_stable_and_distinguishes_inputs() {
        assert_eq!(super::fnv1a(b"cover"), super::fnv1a(b"cover"));
        assert_ne!(super::fnv1a(b"cover-a"), super::fnv1a(b"cover-b"));
        assert_eq!(
            super::fnv1a(b""),
            0xcbf2_9ce4_8422_2325,
            "空输入应当返回偏移基准值"
        );
    }

    #[test]
    fn decoded_cover_is_bgra_not_rgba() {
        use image::GenericImage as _;

        // 源图取 1024 宽：正好落在最高一档，插值不会给逐字节断言带上 ±1 噪声。
        const W: u32 = 1024;
        let mut source = image::DynamicImage::new_rgb8(W, 2);
        for x in 0..W {
            let color = if x < W / 2 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 255])
            };
            source.put_pixel(x, 0, color);
            source.put_pixel(x, 1, color);
        }
        let mut png = Vec::new();
        source
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("编码测试 PNG 不应失败");

        let (frame, bytes) = super::decode_bgra(&png, 512).expect("合法 PNG 应当解得出来");
        let buffer = frame.buffer().as_raw();
        assert_eq!(frame.buffer().width(), 512, "1024×2 应当缩到 512×1");
        assert_eq!(
            bytes,
            buffer.len(),
            "带回来的字节数必须就是这张位图的实际占用，缓存预算全靠它"
        );

        assert_eq!(
            [buffer[0], buffer[1], buffer[2], buffer[3]],
            [0, 0, 255, 255],
            "红色像素的 BGRA 应当是 [0,0,255,255]"
        );
        let blue = 256 * 4;
        assert_eq!(
            [
                buffer[blue],
                buffer[blue + 1],
                buffer[blue + 2],
                buffer[blue + 3]
            ],
            [255, 0, 0, 255],
            "蓝色像素的 BGRA 应当是 [255,0,0,255]"
        );
    }

    #[test]
    fn a_500px_cover_actually_shrinks_to_the_requested_tier() {
        let source = image::DynamicImage::new_rgb8(500, 500);
        let mut png = Vec::new();
        source
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("编码测试 PNG 不应失败");

        for (size, bucket) in [(44.0, 96), (32.0, 64), (200.0, 384)] {
            assert_eq!(
                super::bucket_for(size),
                bucket,
                "{size}px 槽位应当取 {bucket} 档"
            );
            let (frame, _) = super::decode_bgra(&png, bucket).expect("合法 PNG 应当解得出来");
            assert_eq!(
                (frame.buffer().width(), frame.buffer().height()),
                (bucket, bucket),
                "{size}px 槽位拿到的位图必须已经缩到 {bucket}，否则又会在 GPU 里被 5 倍以上缩小"
            );
        }
    }

    #[test]
    fn tiers_keep_the_worst_case_minification_under_2_3x() {
        let mut worst = 0f32;
        for pair in super::BUCKETS.windows(2) {
            let (small, large) = (pair[0] as f32, pair[1] as f32);
            assert!(large > small, "档位必须递增");
            worst = worst.max(large / (small / 1.5));
        }
        assert!(
            worst <= 2.3,
            "最坏情况要把 {worst:.2} 倍缩小交给没有 mip 链的双线性，会重新出现色块"
        );
        assert_eq!(super::bucket_for(9999.0), 512);
        assert_eq!(super::bucket_for(1.0), 64);
    }

    #[test]
    fn each_slot_in_the_layout_maps_to_the_expected_tier() {
        for (label, size, tier) in [
            ("侧栏行", 32.0, 64),
            ("列表行", 40.0, 64),
            ("下一首", 44.0, 96),
            ("迷你条", 56.0, 96),
            ("详情页头图", 180.0, 384),
            ("正在播放", 200.0, 384),
        ] {
            assert_eq!(super::bucket_for(size), tier, "{label}（{size}px）");
        }
    }

    #[test]
    fn the_same_cover_is_cached_per_tier() {
        let id = lyre_core::TrackId::from_path(std::path::Path::new("x.flac"));
        let small = super::bucket_for(44.0);
        let large = super::bucket_for(200.0);
        assert_ne!(small, large, "44 与 200 应当落在不同档");
        let a = super::CoverKey::Track(id, small);
        let b = super::CoverKey::Track(id, large);
        assert_ne!(a, b);
    }

    fn decoded(hash: u64, bucket: u32) -> super::Decoded {
        let frame = image::Frame::new(image::RgbaImage::from_pixel(
            bucket,
            bucket,
            image::Rgba([0, 0, 0, 255]),
        ));
        let bytes = frame.buffer().as_raw().len();
        ((hash, bucket), frame, bytes)
    }

    /// 按 `ready_file` 相同的方式造键，淘汰后可以回查同一个槽位。
    fn slot_of(hash: u64, bucket: u32) -> super::CoverKey {
        super::CoverKey::File(
            lyre_core::TrackId::from_path(std::path::Path::new(&format!("{hash}.png"))),
            bucket,
        )
    }

    #[test]
    fn the_budget_is_measured_in_bytes_not_in_bitmaps() {
        let mut cache = super::CoverCache {
            limit: 3 * 1024 * 1024,
            ..Default::default()
        };
        let mut evicted = Vec::new();
        for hash in 0..5u64 {
            evicted.extend(cache.store_key(slot_of(hash, 512), Some(decoded(hash, 512))));
        }

        assert!(
            cache.bytes <= cache.limit,
            "装了 5 张 1MB 之后还占 {}MB，预算没起作用",
            cache.bytes / 1_048_576
        );
        assert_eq!(
            cache.images.len(),
            3,
            "1MB 一张、预算 3MB，应当只留最近三张"
        );
        assert_eq!(
            evicted.len(),
            2,
            "淘汰下来的两张要交给调用方去还显存，不能只从表里摘掉就算完"
        );
        assert!(
            cache
                .ready_file(std::path::Path::new("0.png"), 341.0)
                .is_none(),
            "最旧那张应当已被淘汰，且指向它的槽位一起清掉（否则取图会拿到悬空引用）"
        );
        assert!(
            cache
                .ready_file(std::path::Path::new("4.png"), 341.0)
                .is_some(),
            "刚存进去的那张不该被连累"
        );
    }

    #[test]
    fn one_oversized_bitmap_still_gets_kept() {
        let mut cache = super::CoverCache {
            limit: 1024,
            ..Default::default()
        };
        cache.store_key(slot_of(1, 64), Some(decoded(1, 64)));

        assert_eq!(cache.images.len(), 1, "至少得留一张");
        assert!(
            cache
                .ready_file(std::path::Path::new("1.png"), 32.0)
                .is_some()
        );
    }
}
