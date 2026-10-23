SELECT
        (SELECT COUNT(*) FROM media_metadata WHERE item_type IN ('Movie', 'Series')) AS media_cached_total,
        (SELECT COUNT(*) FROM p115_mediainfo_cache) AS mediainfo_backed_up_total,
        (SELECT COALESCE(SUM(hit_count), 0) FROM p115_mediainfo_cache) AS mediainfo_hits_total
