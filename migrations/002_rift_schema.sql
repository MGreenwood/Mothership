-- Create rifts table
CREATE TABLE rifts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name VARCHAR(64) NOT NULL,
    description TEXT,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    author UUID NOT NULL REFERENCES users(id),
    is_conflict_rift BOOLEAN NOT NULL DEFAULT FALSE,
    UNIQUE (project_id, name)
);

-- Add new fields to rifts table
ALTER TABLE rifts
    ADD COLUMN IF NOT EXISTS description TEXT,
    ADD COLUMN IF NOT EXISTS author UUID REFERENCES users(id),
    ADD COLUMN IF NOT EXISTS is_conflict_rift BOOLEAN NOT NULL DEFAULT FALSE;

-- Create rift_files table to track files in each rift
CREATE TABLE IF NOT EXISTS rift_files (
    rift_id UUID NOT NULL REFERENCES rifts(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    hash TEXT NOT NULL,
    PRIMARY KEY (rift_id, path)
);

-- Create user_rift_state table to track which rift each user is currently in
CREATE TABLE IF NOT EXISTS user_rift_state (
    user_id UUID NOT NULL REFERENCES users(id),
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    current_rift_id UUID NOT NULL REFERENCES rifts(id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, project_id)
);

-- Create indexes for better query performance
CREATE INDEX idx_rifts_project_id ON rifts(project_id);
CREATE INDEX IF NOT EXISTS idx_rift_files_rift_id ON rift_files(rift_id);
CREATE INDEX IF NOT EXISTS idx_user_rift_state_user_project ON user_rift_state(user_id, project_id);

-- Create a function to calculate line differences between file versions
CREATE OR REPLACE FUNCTION diff_lines(old_hash TEXT, new_hash TEXT)
RETURNS TEXT[] AS $$
BEGIN
    -- This is a placeholder. In a real implementation, this would:
    -- 1. Fetch file contents from both hashes
    -- 2. Calculate line-by-line differences
    -- 3. Return array of changed line numbers
    -- For now, we'll just return an array with a single element
    RETURN ARRAY['1'];
END;
$$ LANGUAGE plpgsql; 