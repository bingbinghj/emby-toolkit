WITH date_range AS (
            SELECT generate_series(
                (NOW() AT TIME ZONE $1)::date - ($2::int - 1),
                (NOW() AT TIME ZONE $1)::date,
                interval '1 day'
            )::date AS stat_date
        ),
        raw_assets AS (
            SELECT
                mm.item_type,
                asset ->> 'date_added_to_library' AS added_text
            FROM media_metadata mm
            CROSS JOIN LATERAL jsonb_array_elements(mm.asset_details_json) AS asset
            WHERE
                mm.in_library = TRUE
                AND mm.item_type IN ('Movie', 'Episode')
                AND mm.asset_details_json IS NOT NULL
                AND jsonb_typeof(mm.asset_details_json) = 'array'
                AND jsonb_array_length(mm.asset_details_json) > 0
                AND mm.asset_details_json::text LIKE $3
                AND asset ? 'date_added_to_library'
                AND NULLIF(asset ->> 'date_added_to_library', '') IS NOT NULL
        ),
        asset_rows AS (
            SELECT
                (
                    CASE
                        WHEN added_text ~ '(Z|[+-][0-9]{2}:?[0-9]{2})$'
                            THEN added_text::timestamptz
                        ELSE added_text::timestamp AT TIME ZONE 'UTC'
                    END
                    AT TIME ZONE $1
                )::date AS stat_date,
                item_type
            FROM raw_assets
            WHERE added_text ~ '^[0-9]{4}-[0-9]{2}-[0-9]{2}'
        )
        SELECT
            d.stat_date::text AS date,
            to_char(d.stat_date, 'MM-DD') AS date_label,
            COALESCE(COUNT(a.stat_date), 0)::int AS count,
            COALESCE(COUNT(a.stat_date) FILTER (WHERE a.item_type = 'Movie'), 0)::int AS movies,
            COALESCE(COUNT(a.stat_date) FILTER (WHERE a.item_type = 'Episode'), 0)::int AS episodes
        FROM date_range d
        LEFT JOIN asset_rows a ON a.stat_date = d.stat_date
        GROUP BY d.stat_date
        ORDER BY d.stat_date ASC
