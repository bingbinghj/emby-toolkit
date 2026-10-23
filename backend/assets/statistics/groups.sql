SELECT
            release_group,
            COUNT(*) AS count
        FROM (
            SELECT
                jsonb_array_elements_text(asset -> 'release_group_raw') AS release_group
            FROM (
                SELECT jsonb_array_elements(asset_details_json) AS asset
                FROM media_metadata
                WHERE
                    in_library = TRUE
                    AND asset_details_json IS NOT NULL
                    AND jsonb_array_length(asset_details_json) > 0
                    -- 仍然检查 date_added_to_library 字段是否存在，以确保是有效入库记录
                    AND asset_details_json::text LIKE $1
            ) AS assets
        ) AS release_groups
        WHERE
            release_group IS NOT NULL AND release_group != ''
        GROUP BY release_group
        ORDER BY count DESC
        LIMIT $2
