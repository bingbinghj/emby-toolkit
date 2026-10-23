CREATE TABLE IF NOT EXISTS processed_log (
                        item_id TEXT PRIMARY KEY,
                        item_name TEXT,
                        processed_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
                        score REAL,
                        assets_synced_at TIMESTAMP WITH TIME ZONE,
                        last_emby_modified_at TIMESTAMP WITH TIME ZONE
                    );

CREATE TABLE IF NOT EXISTS failed_log (
                        item_id TEXT PRIMARY KEY,
                        item_name TEXT,
                        reason TEXT,
                        failed_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
                        error_message TEXT,
                        item_type TEXT,
                        score REAL
                    );

CREATE TABLE IF NOT EXISTS translation_cache (
                        original_text TEXT PRIMARY KEY,
                        translated_text TEXT,
                        engine_used TEXT,
                        last_updated_at TIMESTAMP WITH TIME ZONE DEFAULT NOW()
                    );

CREATE TABLE IF NOT EXISTS app_settings (
                        setting_key TEXT PRIMARY KEY,
                        value_json JSONB,
                        last_updated_at TIMESTAMP WITH TIME ZONE DEFAULT NOW()
                    );

CREATE TABLE IF NOT EXISTS emby_users (
                        id TEXT PRIMARY KEY,
                        name TEXT NOT NULL,
                        is_administrator BOOLEAN,
                        last_seen_at TIMESTAMP WITH TIME ZONE,
                        profile_image_tag TEXT,
                        policy_json JSONB,
                        last_updated_at TIMESTAMP WITH TIME ZONE DEFAULT NOW()
                    );

CREATE TABLE IF NOT EXISTS media_metadata (
                        -- 核心标识符
                        tmdb_id TEXT NOT NULL,
                        item_type TEXT NOT NULL, -- 'Movie', 'Series', 'Season', 'Episode'

                        -- 媒体库状态
                        in_library BOOLEAN DEFAULT FALSE NOT NULL,
                        emby_item_ids_json JSONB NOT NULL DEFAULT '[]'::jsonb,
                        file_sha1_json JSONB NOT NULL DEFAULT '[]'::jsonb,
                        file_pickcode_json JSONB NOT NULL DEFAULT '[]'::jsonb,
                        date_added TIMESTAMP WITH TIME ZONE,
                        asset_details_json JSONB,

                        -- 核心与扩展元数据
                        imdb_id TEXT,
                        title TEXT,
                        original_title TEXT,
                        original_language TEXT,
                        overview TEXT,
                        tagline TEXT,
                        release_date DATE,
                        release_year INTEGER,
                        last_air_date DATE,
                        poster_path TEXT,
                        backdrop_path TEXT,
                        homepage TEXT,
                        runtime_minutes INTEGER,
                        rating REAL,
                        official_rating_json JSONB,
                        custom_rating TEXT,
                        genres_json JSONB,
                        actors_json JSONB,
                        directors_json JSONB,
                        production_companies_json JSONB,
                        networks_json JSONB,
                        countries_json JSONB,
                        keywords_json JSONB,
                        last_updated_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
                        tags_json JSONB,

                        -- 剧集专属与层级数据
                        parent_series_tmdb_id TEXT,
                        season_number INTEGER,
                        episode_number INTEGER,
                        episode_group_id TEXT,

                        last_episode_to_air_json JSONB,
                        total_episodes INTEGER DEFAULT 0,
                        total_episodes_locked BOOLEAN DEFAULT FALSE,

                        -- 内部管理字段
                        last_synced_at TIMESTAMP WITH TIME ZONE,
                        created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(),

                        -- 主键
                        PRIMARY KEY (tmdb_id, item_type)
                    );

CREATE TABLE IF NOT EXISTS person_metadata (
                        -- 核心身份标识 (建议以 TMDb ID 为主键，因为影视刮削高度依赖 TMDb)
                        tmdb_person_id INTEGER PRIMARY KEY,

                        -- 其他平台的 ID 映射
                        imdb_id TEXT UNIQUE,
                        douban_celebrity_id TEXT UNIQUE,

                        -- 基础元数据
                        primary_name TEXT NOT NULL,       -- 主要译名 (原 map 表字段)
                        original_name TEXT,               -- 原名 (原 metadata 表字段)
                        profile_path TEXT,                -- 头像路径
                        gender INTEGER,                   -- 性别
                        adult BOOLEAN,                    -- 是否成人影星
                        popularity REAL,                  -- 热度

                        -- 时间戳
                        last_synced_at TIMESTAMP WITH TIME ZONE,
                        last_updated_at TIMESTAMP WITH TIME ZONE DEFAULT NOW()
                    );

CREATE TABLE IF NOT EXISTS emby_users_extended (
                        emby_user_id TEXT PRIMARY KEY,
                        status TEXT NOT NULL DEFAULT 'active', -- active(激活), expired(过期), disabled(禁用)
                        registration_date TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
                        expiration_date TIMESTAMP WITH TIME ZONE, -- 核心字段：用户的到期时间
                        notes TEXT,
                        created_by TEXT DEFAULT 'self-registered', -- 'self-registered' 或 'admin'
                        FOREIGN KEY(emby_user_id) REFERENCES emby_users(id) ON DELETE CASCADE
                    );

CREATE TABLE IF NOT EXISTS p115_filesystem_cache (
                        id TEXT PRIMARY KEY,           -- 115 的 cid (文件夹) 或 fid (文件)
                        parent_id TEXT NOT NULL,       -- 父目录 ID (根目录为 '0')
                        name TEXT NOT NULL,            -- 文件/文件夹名称
                        local_path TEXT,               -- 本地映射路径 (如果已同步到本地)
                        sha1 TEXT,
                        pick_code TEXT,
                        preid TEXT,               -- 文件前 128KB SHA1，用于指纹/媒体信息缓存
                        size BIGINT DEFAULT 0,
                        updated_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(), -- 最后同步时间

                        -- 复合唯一约束：同一个父目录下不能有同名文件 (用于快速查找)
                        -- 注意：115 实际上允许同名，但在我们的管理逻辑中通常假设唯一，或者只缓存最新的
                        CONSTRAINT uniq_p115_parent_name UNIQUE (parent_id, name)
                    );

CREATE TABLE IF NOT EXISTS p115_mediainfo_cache (
                        sha1 TEXT PRIMARY KEY,
                        mediainfo_json JSONB,
                        raw_ffprobe_json JSONB,
                        created_at TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
                        hit_count INTEGER DEFAULT 0
                    );

CREATE INDEX IF NOT EXISTS idx_mm_in_library ON media_metadata (in_library);

CREATE INDEX IF NOT EXISTS idx_mm_release_year ON media_metadata (release_year);

CREATE INDEX IF NOT EXISTS idx_mm_parent_series ON media_metadata (parent_series_tmdb_id);

CREATE INDEX IF NOT EXISTS idx_mm_emby_ids_gin ON media_metadata USING GIN(emby_item_ids_json);

CREATE INDEX IF NOT EXISTS idx_mm_parent_series_season ON media_metadata (parent_series_tmdb_id, season_number);

CREATE INDEX IF NOT EXISTS idx_mm_item_type ON media_metadata (item_type);

CREATE INDEX IF NOT EXISTS idx_mm_asset_details_gin ON media_metadata USING GIN(asset_details_json);

CREATE INDEX IF NOT EXISTS idx_mm_file_sha1_json_gin ON media_metadata USING GIN(file_sha1_json);

CREATE INDEX IF NOT EXISTS idx_mm_file_pickcode_json_gin ON media_metadata USING GIN(file_pickcode_json);

CREATE INDEX IF NOT EXISTS idx_mm_episode_parent_library ON media_metadata (parent_series_tmdb_id, item_type, in_library) WHERE item_type = 'Episode';

CREATE INDEX IF NOT EXISTS idx_mm_genres_gin ON media_metadata USING GIN(genres_json jsonb_path_ops);

CREATE INDEX IF NOT EXISTS idx_mm_tags_gin ON media_metadata USING GIN(tags_json jsonb_path_ops);

CREATE INDEX IF NOT EXISTS idx_mm_countries_gin ON media_metadata USING GIN(countries_json jsonb_path_ops);

CREATE INDEX IF NOT EXISTS idx_mm_companies_gin ON media_metadata USING GIN(production_companies_json jsonb_path_ops);

CREATE INDEX IF NOT EXISTS idx_mm_networks_gin ON media_metadata USING GIN(networks_json jsonb_path_ops);

CREATE INDEX IF NOT EXISTS idx_mm_keywords_gin ON media_metadata USING GIN(keywords_json jsonb_path_ops);

CREATE INDEX IF NOT EXISTS idx_mm_asset_details_gin ON media_metadata USING GIN(asset_details_json);

CREATE INDEX IF NOT EXISTS idx_mm_directors_gin ON media_metadata USING GIN(directors_json jsonb_path_ops);

CREATE INDEX IF NOT EXISTS idx_mm_actors_gin ON media_metadata USING GIN(actors_json jsonb_path_ops);

CREATE INDEX IF NOT EXISTS idx_mm_date_added_desc ON media_metadata (date_added DESC);

CREATE INDEX IF NOT EXISTS idx_mm_rating_desc ON media_metadata (rating DESC);

CREATE INDEX IF NOT EXISTS idx_mm_release_date_desc ON media_metadata (release_date DESC);

CREATE INDEX IF NOT EXISTS idx_p115_parent_id ON p115_filesystem_cache (parent_id);

CREATE INDEX IF NOT EXISTS idx_p115_name ON p115_filesystem_cache (name);

CREATE INDEX IF NOT EXISTS idx_p115_sha1_upper ON p115_filesystem_cache (UPPER(sha1)) WHERE sha1 IS NOT NULL AND sha1 <> '';

CREATE INDEX IF NOT EXISTS idx_p115_pick_code ON p115_filesystem_cache (pick_code) WHERE pick_code IS NOT NULL AND pick_code <> '';

CREATE INDEX IF NOT EXISTS idx_p115_preid ON p115_filesystem_cache (preid) WHERE preid IS NOT NULL AND preid <> '';

CREATE INDEX IF NOT EXISTS idx_p115_local_path ON p115_filesystem_cache (local_path) WHERE local_path IS NOT NULL AND local_path <> '';

CREATE INDEX IF NOT EXISTS idx_mm_type_parent ON media_metadata (item_type, parent_series_tmdb_id);

CREATE INDEX IF NOT EXISTS idx_mm_episode_parent_season_ep_library ON media_metadata(parent_series_tmdb_id, season_number, episode_number) WHERE item_type='Episode' AND in_library=TRUE;

ALTER TABLE emby_users_extended ADD COLUMN IF NOT EXISTS telegram_chat_id TEXT;
