-- Client-local workspace layout templates/instances are versioned domain data.
-- Keep the legacy normalized tab/pane/document columns during the Develop
-- migration so existing sessions restore without loss.
ALTER TABLE app ADD COLUMN layout_json TEXT;
