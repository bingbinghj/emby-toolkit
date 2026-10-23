SELECT
            release_group,
            COUNT(*) AS count
        FROM (
            SELECT
                jsonb_array_elements_text(asset -> 'release_group_raw') AS release_group,
                ((asset ->> 'date_added_to_library')::timestamp AT TIME ZONE 'UTC') AS asset_added_at_utc
            FROM (
                SELECT jsonb_array_elements(asset_details_json) AS asset
                FROM media_metadata
                WHERE
                    in_library = TRUE
                    AND asset_details_json IS NOT NULL
                    AND jsonb_array_length(asset_details_json) > 0
                    AND asset_details_json::text LIKE $1
            ) AS assets
        ) AS release_groups
        WHERE
            release_group IS NOT NULL AND release_group != ''
            AND (asset_added_at_utc AT TIME ZONE $2)::date = (NOW() AT TIME ZONE $3)::date
        GROUP BY release_group
        ORDER BY count DESC
        LIMIT $4
