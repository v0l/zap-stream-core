-- Add node name column to user_stream table for horizontal scaling support
-- This allows tracking which node instance is handling each stream

ALTER TABLE user_stream 
ADD COLUMN node_name VARCHAR(255) DEFAULT NULL 
COMMENT 'Name of the node handling this stream for horizontal scaling';

-- Add index for efficient querying of streams by node
CREATE INDEX idx_user_stream_node_name ON user_stream(node_name);