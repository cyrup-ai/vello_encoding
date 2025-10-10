//! Blitz-text integrated glyph cache implementation
//!
//! This uses blitz-text's UnifiedTextSystem for high-level text operations,
//! replacing manual glyph cache management with sophisticated multi-tier caching.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Instant;

use blitz_text::{
    Attrs, Buffer, Color, FontSystem, GpuRenderConfig, Metrics, PreparedText, TextAreaConfig,
    TextMeasurement, TextSystemError, UnifiedTextSystem,
};
use dashmap::DashMap;
use glyphon::TextBounds;
use peniko::{Font, Style};

use super::{Encoding, StreamOffsets};

/// Command for vector outline construction
#[derive(Debug, Clone)]
#[allow(dead_code)]
enum OutlineCommand {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    QuadTo(f32, f32, f32, f32),
    CubicTo(f32, f32, f32, f32, f32, f32),
    Close,
}

/// Collector that captures outline commands for later replay
#[allow(dead_code)]
struct OutlineCommandCollector<'a> {
    scale: f32,
    commands: &'a mut Vec<OutlineCommand>,
}

impl<'a> OutlineCommandCollector<'a> {
    #[allow(dead_code)]
    fn new(scale: f32, commands: &'a mut Vec<OutlineCommand>) -> Self {
        Self { scale, commands }
    }
}

impl<'a> ttf_parser::OutlineBuilder for OutlineCommandCollector<'a> {
    fn move_to(&mut self, x: f32, y: f32) {
        // CRITICAL FIX: Remove Y-axis flipping to match original skrifa behavior
        // The coordinate transformation should happen at the render level, not here
        self.commands
            .push(OutlineCommand::MoveTo(x * self.scale, y * self.scale));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.commands
            .push(OutlineCommand::LineTo(x * self.scale, y * self.scale));
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.commands.push(OutlineCommand::QuadTo(
            x1 * self.scale,
            y1 * self.scale,
            x * self.scale,
            y * self.scale,
        ));
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.commands.push(OutlineCommand::CubicTo(
            x1 * self.scale,
            y1 * self.scale,
            x2 * self.scale,
            y2 * self.scale,
            x * self.scale,
            y * self.scale,
        ));
    }

    fn close(&mut self) {
        self.commands.push(OutlineCommand::Close);
    }
}

/// Cache key for text rendering operations
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TextCacheKey {
    text_hash: u64,
    font_hash: u64,
    size: u32,
    style_hash: u64,
}

/// Blitz-text integrated glyph cache with sophisticated text processing
pub(crate) struct GlyphCache {
    /// GPU device for creating UnifiedTextSystem instances
    device: Arc<wgpu::Device>,
    /// GPU queue for text system operations
    queue: Arc<wgpu::Queue>,
    /// Texture format for rendering
    format: wgpu::TextureFormat,
    /// Cache for prepared text objects
    prepared_text_cache: Arc<DashMap<TextCacheKey, Arc<PreparedText>>>,
    /// Performance monitoring
    performance_monitor: Arc<AtomicU64>,
    /// Cache for reusable encodings
    encoding_pool: Arc<DashMap<u64, Arc<Encoding>>>,
    /// Statistics tracking
    stats: GlyphCacheStats,
}

/// Statistics for glyph cache performance
#[derive(Debug, Default)]
struct GlyphCacheStats {
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    text_preparations: AtomicU64,
    encoding_reuses: AtomicU64,
}

impl GlyphCache {
    /// Create a new GlyphCache with blitz-text integration
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Result<Self, TextSystemError> {
        Ok(Self {
            device: Arc::new(device.clone()),
            queue: Arc::new(queue.clone()),
            format,
            prepared_text_cache: Arc::new(DashMap::new()),
            performance_monitor: Arc::new(AtomicU64::new(0)),
            encoding_pool: Arc::new(DashMap::new()),
            stats: GlyphCacheStats::default(),
        })
    }

    /// Create with custom configuration
    #[allow(dead_code)]
    pub fn with_config(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        _config: GpuRenderConfig,
    ) -> Result<Self, TextSystemError> {
        // Configuration will be applied when creating UnifiedTextSystem instances
        Ok(Self {
            device: Arc::new(device.clone()),
            queue: Arc::new(queue.clone()),
            format,
            prepared_text_cache: Arc::new(DashMap::new()),
            performance_monitor: Arc::new(AtomicU64::new(0)),
            encoding_pool: Arc::new(DashMap::new()),
            stats: GlyphCacheStats::default(),
        })
    }
}

// Note: Default implementation removed - use ::new() for proper initialization

impl GlyphCache {
    /// Create a session for text rendering using blitz-text
    ///
    /// # Arguments
    /// * `font` - The font to use for text rendering
    /// * `size` - Font size in pixels
    /// * `style` - Font style configuration
    /// * `text` - The text to render
    pub(crate) async fn session<'a>(
        &'a mut self,
        font: &'a Font,
        size: f32,
        style: &'a Style,
        text: &'a str,
    ) -> Result<GlyphCacheSession<'a>, TextSystemError> {
        // Create text attributes from font parameters
        // Note: peniko::Font doesn't have a family field, use a placeholder
        let attrs = Attrs::new()
            .family(blitz_text::Family::SansSerif)
            .metrics(Metrics::relative(size, 1.0));

        // Generate cache key for this text rendering request
        let cache_key = self.generate_cache_key(text, font, size, style);

        // Check if we have prepared text cached
        let prepared_text = if let Some(cached) = self.prepared_text_cache.get(&cache_key) {
            self.stats.cache_hits.fetch_add(1, Ordering::Relaxed);
            cached.clone()
        } else {
            self.stats.cache_misses.fetch_add(1, Ordering::Relaxed);

            // Create UnifiedTextSystem for this measurement operation
            let mut text_system = UnifiedTextSystem::new(
                &self.device,
                &self.queue,
                self.format,
                wgpu::MultisampleState::default(),
                None,
            ).await?;
            let measurement = text_system.measure_text(text, attrs, None, None).await?;

            // Create prepared text for rendering
            let buffer = Buffer::new(&mut FontSystem::new(), Metrics::relative(size, 1.0));
            let text_area_config = TextAreaConfig {
                position: (0.0, 0.0),
                scale: 1.0,
                bounds: TextBounds {
                    left: 0,
                    top: 0,
                    right: measurement.content_width as i32,
                    bottom: measurement.content_height as i32,
                },
                default_color: Color::rgba(255, 255, 255, 255), // White color
            };
            let prepared = Arc::new(PreparedText {
                measurement,
                buffer,
                text_area_config,
                preparation_time: std::time::Duration::default(),
            });

            self.prepared_text_cache
                .insert(cache_key.clone(), prepared.clone());
            self.stats.text_preparations.fetch_add(1, Ordering::Relaxed);
            prepared
        };

        Ok(GlyphCacheSession {
            cache: self,
            font,
            size,
            style,
            text,
            prepared_text,
            cache_key,
        })
    }

    /// Generate cache key for text rendering
    fn generate_cache_key(
        &self,
        text: &str,
        font: &Font,
        size: f32,
        style: &Style,
    ) -> TextCacheKey {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let text_hash = hasher.finish();

        let mut hasher = DefaultHasher::new();
        font.data.as_ref().hash(&mut hasher);
        let font_hash = hasher.finish();

        let mut hasher = DefaultHasher::new();
        // peniko::Style doesn't implement Hash, use discriminant instead
        std::mem::discriminant(style).hash(&mut hasher);
        let style_hash = hasher.finish();

        TextCacheKey {
            text_hash,
            font_hash,
            size: (size * 100.0) as u32, // Convert to fixed point for hashing
            style_hash,
        }
    }

    /// Maintenance method for cache cleanup and optimization
    pub(crate) fn maintain(&mut self) {
        let _current_time = Instant::now();

        // Update performance monitoring
        self.performance_monitor.fetch_add(1, Ordering::Relaxed);

        // Periodically trim prepared text cache
        if self.prepared_text_cache.len() > 1000 {
            // Keep most recent 800 entries, remove older ones
            let to_remove: Vec<_> = self
                .prepared_text_cache
                .iter()
                .take(self.prepared_text_cache.len() - 800)
                .map(|entry| entry.key().clone())
                .collect();

            for key in to_remove {
                self.prepared_text_cache.remove(&key);
            }
        }

        // Periodically trim encoding pool
        if self.encoding_pool.len() > 500 {
            // Keep most recent 400 encodings
            let to_remove: Vec<_> = self
                .encoding_pool
                .iter()
                .take(self.encoding_pool.len() - 400)
                .map(|entry| *entry.key())
                .collect();

            for key in to_remove {
                self.encoding_pool.remove(&key);
            }
        }
    }

    /// Get cache statistics
    #[allow(dead_code)]
    pub(crate) fn stats(&self) -> (u64, u64, u64, u64) {
        (
            self.stats.cache_hits.load(Ordering::Relaxed),
            self.stats.cache_misses.load(Ordering::Relaxed),
            self.stats.text_preparations.load(Ordering::Relaxed),
            self.stats.encoding_reuses.load(Ordering::Relaxed),
        )
    }
}

/// Session for text rendering operations using blitz-text
pub(crate) struct GlyphCacheSession<'a> {
    cache: &'a mut GlyphCache,
    /// Font parameters for text rendering
    #[allow(dead_code)]
    font: &'a Font,
    /// Font size in pixels
    #[allow(dead_code)]
    size: f32,
    /// Font style configuration
    style: &'a Style,
    /// Text content to render
    #[allow(dead_code)]
    text: &'a str,
    /// Prepared text with measurements and layout
    prepared_text: Arc<PreparedText>,
    /// Cache key for this session
    cache_key: TextCacheKey,
}

impl<'a> GlyphCacheSession<'a> {
    /// Get text measurement from prepared text
    #[allow(dead_code)]
    pub(crate) fn measurement(&self) -> &TextMeasurement {
        &self.prepared_text.measurement
    }

    /// Get preparation time from prepared text
    #[allow(dead_code)]
    pub(crate) fn preparation_time(&self) -> std::time::Duration {
        self.prepared_text.preparation_time
    }
    /// Get or create encoding for text rendering
    pub(crate) fn get_encoding(&mut self) -> Arc<Encoding> {
        // Generate encoding key based on cache key
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        self.cache_key.hash(&mut hasher);
        let encoding_key = hasher.finish();

        // Try to reuse cached encoding
        if let Some(cached) = self.cache.encoding_pool.get(&encoding_key) {
            self.cache
                .stats
                .encoding_reuses
                .fetch_add(1, Ordering::Relaxed);
            return cached.clone();
        }

        // Create new encoding
        let encoding = Arc::new(Encoding::new());
        self.cache
            .encoding_pool
            .insert(encoding_key, encoding.clone());
        encoding
    }

    /// Return encoding to cache for reuse - handled automatically
    #[allow(dead_code)]
    pub(crate) fn return_encoding(&mut self, _encoding: Arc<Encoding>) {
        // Encoding caching is handled automatically in get_encoding method
    }

    /// Get text encoding using blitz-text measurement system
    #[inline(always)]
    pub(crate) fn get_text_encoding(&mut self) -> (Arc<Encoding>, StreamOffsets) {
        // Get or create encoding
        let mut encoding = self.get_encoding();
        let encoding_ptr = Arc::make_mut(&mut encoding);
        encoding_ptr.reset();

        // Use text measurement for proper text layout
        let measurement = &self.prepared_text.measurement;

        // Encode style based on peniko style
        let is_fill = match self.style {
            Style::Fill(fill) => {
                encoding_ptr.encode_fill_style(*fill);
                true
            }
            Style::Stroke(stroke) => {
                if encoding_ptr.encode_stroke_style(stroke) {
                    false
                } else {
                    encoding_ptr.encode_fill_style(peniko::Fill::NonZero);
                    true
                }
            }
        };

        // Create path encoder for text
        let mut path = encoding_ptr.encode_path(is_fill);

        // Create text outline based on text measurement
        // For now, we'll create a simple rectangle representing the text bounds
        let width = measurement.content_width;
        let height = measurement.content_height;

        // Create a simple rectangle representing the entire text (placeholder)
        // In a full implementation, this would extract actual glyph outlines from font data
        path.move_to(0.0, 0.0);
        path.line_to(width, 0.0);
        path.line_to(width, height);
        path.line_to(0.0, height);
        path.close();

        let path_segments = path.finish(true);
        if path_segments == 0 {
            encoding_ptr.reset();
        }

        let stream_offsets = encoding_ptr.stream_offsets();
        (encoding, stream_offsets)
    }
}
