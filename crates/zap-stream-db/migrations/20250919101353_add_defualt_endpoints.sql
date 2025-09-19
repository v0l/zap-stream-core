-- Add default ingest endpoints only if no endpoints exist
INSERT INTO ingest_endpoint (name, cost, capabilities)
SELECT * FROM (
                  SELECT 'Good' as name, 2500 as cost, 'variant:2160:20000000,variant:1440:12000000,variant:1080:8000000,variant:720:4000000,variant:480:1500000' as capabilities
                  UNION ALL
                  SELECT 'Basic' as name, 0 as cost, 'variant:source' as capabilities
              ) AS tmp
WHERE NOT EXISTS (
    SELECT 1 FROM ingest_endpoint
);