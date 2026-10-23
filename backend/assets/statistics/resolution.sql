SELECT
            -- 提取 asset_details_json 数组中第一个元素的 resolution_display 字段
            (jsonb_array_elements(asset_details_json) ->> 'resolution_display') as resolution,
            COUNT(*) as count
        FROM
            media_metadata
        WHERE
            in_library = TRUE
            AND item_type IN ('Movie', 'Episode')
            AND asset_details_json IS NOT NULL
            AND jsonb_array_length(asset_details_json) > 0
        GROUP BY
            resolution
        ORDER BY
            count DESC
