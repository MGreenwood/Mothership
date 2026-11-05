use anyhow::Result;
use chrono::{DateTime, Utc};
use mothership_common::{
    merge::Resolution, protocol::ChatMessage, Project, ProjectId, ProjectSettings, Rift, RiftId,
    User, UserId, UserRole,
};
use sqlx::PgPool;
use std::collections::HashMap;
use uuid::Uuid;

/// Stored wormhole row, with its source rifts and base file hashes loaded
#[derive(Debug, Clone)]
pub struct WormholeRecord {
    pub id: Uuid,
    pub project_id: ProjectId,
    pub name: String,
    pub status: String,
    pub created_by: UserId,
    pub created_at: DateTime<Utc>,
    pub converted_rift_id: Option<RiftId>,
    /// Map of file path to content-store hash for the base snapshot
    pub base_hashes: HashMap<String, String>,
    pub rift_ids: Vec<RiftId>,
    /// Rifts currently toggled off (muted) in this wormhole
    pub disabled_rift_ids: Vec<RiftId>,
    /// Private wormholes are only visible to their creator
    pub is_private: bool,
}

impl WormholeRecord {
    pub fn is_rift_enabled(&self, rift_id: RiftId) -> bool {
        !self.disabled_rift_ids.contains(&rift_id)
    }
}

type WormholeRow = (Uuid, Uuid, String, String, Uuid, DateTime<Utc>, Option<Uuid>, String, bool);

const WORMHOLE_COLUMNS: &str =
    "id, project_id, name, status, created_by, created_at, converted_rift_id, base_files, is_private";

/// PostgreSQL database implementation
#[derive(Clone)]
pub struct Database {
    pool: PgPool,
}

impl Database {
    /// Create a new database connection pool
    pub async fn new(database_url: &str) -> Result<Self> {
        tracing::info!("Connecting to PostgreSQL database...");
        
        let pool = PgPool::connect(database_url).await?;
        
        tracing::info!("Successfully connected to PostgreSQL database");
        
        Ok(Self { pool })
    }

    /// Run database migrations manually (since we can't use sqlx migrate in Docker build)
    pub async fn ensure_schema(&self) -> Result<()> {
        tracing::info!("Ensuring database schema exists...");
        
        // Create the schema manually using runtime queries
        sqlx::query("CREATE EXTENSION IF NOT EXISTS \"uuid-ossp\"")
            .execute(&self.pool).await?;
            
        // Create user_role enum type (ignore error if it already exists)
        let _ = sqlx::query(r#"
            DO $$ BEGIN
                CREATE TYPE user_role AS ENUM ('user', 'admin', 'super_admin');
            EXCEPTION
                WHEN duplicate_object THEN null;
            END $$;
        "#)
            .execute(&self.pool).await;
            
        // Create users table
        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS users (
                id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
                username VARCHAR(255) NOT NULL UNIQUE,
                email VARCHAR(255) NOT NULL UNIQUE,
                role user_role NOT NULL DEFAULT 'user',
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                
                CONSTRAINT users_username_check CHECK (length(username) >= 1 AND length(username) <= 255),
                CONSTRAINT users_email_check CHECK (email ~* '^[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Z|a-z]{2,}$')
            )
        "#).execute(&self.pool).await?;
        
        // Create projects table  
        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS projects (
                id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
                name VARCHAR(255) NOT NULL,
                description TEXT,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                
                CONSTRAINT projects_name_check CHECK (length(name) >= 1 AND length(name) <= 255)
            )
        "#).execute(&self.pool).await?;
        
        // Create project_members table
        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS project_members (
                project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                role VARCHAR(50) NOT NULL DEFAULT 'member',
                joined_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                
                PRIMARY KEY (project_id, user_id)
            )
        "#).execute(&self.pool).await?;
        
        // Create chat_messages table (rift-scoped, pruned to max_chat_history per rift)
        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS chat_messages (
                id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
                rift_id UUID NOT NULL,
                author_id UUID NOT NULL,
                author_username VARCHAR(255) NOT NULL,
                content TEXT NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
        "#).execute(&self.pool).await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS chat_messages_rift_created_idx ON chat_messages (rift_id, created_at)")
            .execute(&self.pool).await?;
        
        // Wormholes: live merge previews over a set of rifts in one project
        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS wormholes (
                id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
                project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                name VARCHAR(255) NOT NULL,
                status VARCHAR(20) NOT NULL DEFAULT 'active',
                created_by UUID NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                converted_rift_id UUID,
                base_files TEXT NOT NULL DEFAULT '{}'
            )
        "#).execute(&self.pool).await?;

        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS wormhole_rifts (
                wormhole_id UUID NOT NULL REFERENCES wormholes(id) ON DELETE CASCADE,
                rift_id UUID NOT NULL REFERENCES rifts(id) ON DELETE CASCADE,
                enabled BOOLEAN NOT NULL DEFAULT TRUE,
                PRIMARY KEY (wormhole_id, rift_id)
            )
        "#).execute(&self.pool).await?;

        sqlx::query(r#"
            ALTER TABLE wormhole_rifts
            ADD COLUMN IF NOT EXISTS enabled BOOLEAN NOT NULL DEFAULT TRUE
        "#).execute(&self.pool).await?;

        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS wormhole_resolutions (
                wormhole_id UUID NOT NULL REFERENCES wormholes(id) ON DELETE CASCADE,
                path TEXT NOT NULL,
                conflict_id TEXT NOT NULL,
                resolution TEXT NOT NULL,
                PRIMARY KEY (wormhole_id, path, conflict_id)
            )
        "#).execute(&self.pool).await?;

        // Privacy flags. Rifts and projects come from the migrations, so these run after them.
        sqlx::query("ALTER TABLE projects ADD COLUMN IF NOT EXISTS is_private BOOLEAN NOT NULL DEFAULT FALSE")
            .execute(&self.pool).await?;
        sqlx::query("ALTER TABLE rifts ADD COLUMN IF NOT EXISTS is_private BOOLEAN NOT NULL DEFAULT FALSE")
            .execute(&self.pool).await?;
        sqlx::query("ALTER TABLE rifts ADD COLUMN IF NOT EXISTS created_by UUID REFERENCES users(id) ON DELETE SET NULL")
            .execute(&self.pool).await?;
        // Legacy rifts predate ownership. Assign the earliest collaborator as owner; rifts with no collaborators stay ownerless.
        sqlx::query(r#"
            UPDATE rifts r
            SET created_by = (
                SELECT rc.user_id FROM rift_collaborators rc
                WHERE rc.rift_id = r.id
                ORDER BY rc.joined_at ASC, rc.user_id ASC
                LIMIT 1
            )
            WHERE r.created_by IS NULL
        "#).execute(&self.pool).await?;
        sqlx::query("ALTER TABLE wormholes ADD COLUMN IF NOT EXISTS is_private BOOLEAN NOT NULL DEFAULT FALSE")
            .execute(&self.pool).await?;

        // Pending requests from project members who want to join a rift
        sqlx::query(r#"
            CREATE TABLE IF NOT EXISTS rift_access_requests (
                rift_id UUID NOT NULL REFERENCES rifts(id) ON DELETE CASCADE,
                user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                PRIMARY KEY (rift_id, user_id)
            )
        "#).execute(&self.pool).await?;

        tracing::info!("Database schema ready!");
        Ok(())
    }

    /// Store a chat message for a rift and return it with its server-assigned id and timestamp
    pub async fn insert_chat_message(
        &self,
        rift_id: RiftId,
        author_id: UserId,
        author_username: &str,
        content: &str,
    ) -> Result<ChatMessage> {
        let (id, timestamp) = sqlx::query_as::<_, (Uuid, DateTime<Utc>)>(
            "INSERT INTO chat_messages (rift_id, author_id, author_username, content) \
             VALUES ($1, $2, $3, $4) RETURNING id, created_at",
        )
        .bind(rift_id)
        .bind(author_id)
        .bind(author_username)
        .bind(content)
        .fetch_one(&self.pool)
        .await?;

        Ok(ChatMessage {
            id,
            rift_id,
            author_id,
            author_username: author_username.to_string(),
            content: content.to_string(),
            timestamp,
        })
    }

    /// Keep only the newest `keep` messages for a rift
    pub async fn prune_chat_messages(&self, rift_id: RiftId, keep: i64) -> Result<()> {
        sqlx::query(
            "DELETE FROM chat_messages WHERE rift_id = $1 AND id NOT IN (\
                SELECT id FROM chat_messages WHERE rift_id = $1 ORDER BY created_at DESC LIMIT $2\
             )",
        )
        .bind(rift_id)
        .bind(keep)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Return the newest `limit` messages for a rift, oldest first
    pub async fn list_chat_messages(&self, rift_id: RiftId, limit: i64) -> Result<Vec<ChatMessage>> {
        let rows = sqlx::query_as::<_, (Uuid, Uuid, String, String, DateTime<Utc>)>(
            "SELECT id, author_id, author_username, content, created_at FROM (\
                SELECT id, author_id, author_username, content, created_at FROM chat_messages \
                WHERE rift_id = $1 ORDER BY created_at DESC LIMIT $2\
             ) recent ORDER BY created_at ASC",
        )
        .bind(rift_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|(id, author_id, author_username, content, timestamp)| ChatMessage {
                id,
                rift_id,
                author_id,
                author_username,
                content,
                timestamp,
            })
            .collect())
    }

    /// Get all projects a user has access to (simplified version for rift handlers)
    pub async fn get_user_projects(&self, user_id: UserId) -> Result<Vec<Project>> {
        // Get projects where user is a member
        let projects = sqlx::query!(
            r#"
            SELECT p.id, p.name, p.description, p.created_at
            FROM projects p
            INNER JOIN project_members pm ON p.id = pm.project_id
            WHERE pm.user_id = $1
            ORDER BY p.created_at DESC
            "#,
            user_id
        )
        .fetch_all(&self.pool)
        .await?;

        let mut result = Vec::new();

        for project_row in projects {
            // Get project members for each project
            let members = sqlx::query!(
                "SELECT user_id FROM project_members WHERE project_id = $1",
                project_row.id
            )
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|member_row| member_row.user_id)
            .collect();

            result.push(Project {
                id: project_row.id,
                name: project_row.name,
                description: project_row.description.unwrap_or_default(),
                members,
                created_at: project_row.created_at,
                settings: ProjectSettings::default(),
            });
        }

        Ok(result)
    }

    /// Get all rifts for a project
    pub async fn get_project_rifts(&self, project_id: ProjectId) -> Result<Vec<Rift>> {
        let rifts = sqlx::query!(
            r#"
            SELECT r.id, r.project_id, r.name, r.parent_rift_id, r.created_at, r.is_active,
                   ARRAY_AGG(rc.user_id) as collaborators
            FROM rifts r
            LEFT JOIN rift_collaborators rc ON r.id = rc.rift_id
            WHERE r.project_id = $1
            GROUP BY r.id, r.project_id, r.name, r.parent_rift_id, r.created_at, r.is_active
            ORDER BY r.created_at ASC
            "#,
            project_id
        )
        .fetch_all(&self.pool)
        .await?;

        let mut result = Vec::new();
        for row in rifts {
            let collaborators = row.collaborators.unwrap_or_default();
            
            result.push(Rift {
                id: row.id,
                project_id: row.project_id,
                name: row.name,
                parent_rift: row.parent_rift_id,
                collaborators,
                created_at: row.created_at,
                last_checkpoint: None, // TODO: Get from checkpoints
                is_active: row.is_active,
            });
        }

        Ok(result)
    }

    /// Get a specific project
    pub async fn get_project(&self, project_id: ProjectId) -> Result<Option<Project>> {
        let project_row = sqlx::query!(
            "SELECT id, name, description, created_at FROM projects WHERE id = $1",
            project_id
        )
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = project_row {
            // Get project members
            let members = sqlx::query!(
                "SELECT user_id FROM project_members WHERE project_id = $1",
                project_id
            )
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|row| row.user_id)
            .collect();

            Ok(Some(Project {
                id: row.id,
                name: row.name,
                description: row.description.unwrap_or_default(),
                members,
                created_at: row.created_at,
                settings: ProjectSettings::default(),
            }))
        } else {
            Ok(None)
        }
    }

    /// List all projects (for testing)
    pub async fn list_all_projects(&self) -> Result<Vec<Project>> {
        let projects = sqlx::query!(
            "SELECT id, name, description, created_at FROM projects ORDER BY created_at DESC"
        )
        .fetch_all(&self.pool)
        .await?;

        let mut result = Vec::new();
        for row in projects {
            // Get project members for each project
            let members = sqlx::query!(
                "SELECT user_id FROM project_members WHERE project_id = $1",
                row.id
            )
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|member_row| member_row.user_id)
            .collect();

            result.push(Project {
                id: row.id,
                name: row.name,
                description: row.description.unwrap_or_default(),
                members,
                created_at: row.created_at,
                settings: ProjectSettings::default(),
            });
        }

        Ok(result)
    }

    /// Get user by ID
    pub async fn get_user(&self, user_id: UserId) -> Result<Option<User>> {
        let user = sqlx::query!(
            "SELECT id, username, email, role as \"role: UserRole\", created_at FROM users WHERE id = $1",
            user_id
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(user.map(|row| User {
            id: row.id,
            username: row.username,
            email: row.email,
            role: row.role,
            created_at: row.created_at,
        }))
    }

    /// Get user by username
    pub async fn get_user_by_username(&self, username: &str) -> Result<Option<User>> {
        let user = sqlx::query!(
            "SELECT id, username, email, role as \"role: UserRole\", created_at FROM users WHERE username = $1",
            username
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(user.map(|row| User {
            id: row.id,
            username: row.username,
            email: row.email,
            role: row.role,
            created_at: row.created_at,
        }))
    }

    /// Get user by email
    pub async fn get_user_by_email(&self, email: &str) -> Result<Option<User>> {
        let user = sqlx::query!(
            "SELECT id, username, email, role as \"role: UserRole\", created_at FROM users WHERE email = $1",
            email
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(user.map(|row| User {
            id: row.id,
            username: row.username,
            email: row.email,
            role: row.role,
            created_at: row.created_at,
        }))
    }

    /// Whether a rift with this name already exists in the project
    pub async fn rift_name_exists(&self, project_id: ProjectId, name: &str) -> Result<bool> {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM rifts WHERE project_id = $1 AND name = $2)",
        )
        .bind(project_id)
        .bind(name)
        .fetch_one(&self.pool)
        .await?;
        Ok(exists)
    }

    /// Create a new rift for a user in a project
    pub async fn create_rift(
        &self,
        project_id: ProjectId,
        user_id: UserId,
        rift_name: Option<String>,
    ) -> Result<Rift> {
        self.create_rift_with_privacy(project_id, user_id, rift_name, false).await
    }

    /// Create a rift owned by `user_id`; private rifts are hidden from non-collaborators
    pub async fn create_rift_with_privacy(
        &self,
        project_id: ProjectId,
        user_id: UserId,
        rift_name: Option<String>,
        is_private: bool,
    ) -> Result<Rift> {
        let rift_id = Uuid::new_v4();
        let name = rift_name.unwrap_or_else(|| "main".to_string());
        
        // Create the rift
        sqlx::query(
            "INSERT INTO rifts (id, project_id, name, is_active, is_private, created_by) VALUES ($1, $2, $3, true, $4, $5)",
        )
        .bind(rift_id)
        .bind(project_id)
        .bind(&name)
        .bind(is_private)
        .bind(user_id)
        .execute(&self.pool)
        .await?;

        // Add user as collaborator
        sqlx::query!(
            r#"
            INSERT INTO rift_collaborators (rift_id, user_id)
            VALUES ($1, $2)
            ON CONFLICT (rift_id, user_id) DO NOTHING
            "#,
            rift_id,
            user_id
        )
        .execute(&self.pool)
        .await?;

        // Return the created rift
        Ok(Rift {
            id: rift_id,
            project_id,
            name,
            parent_rift: None,
            collaborators: vec![user_id],
            created_at: Utc::now(),
            last_checkpoint: None,
            is_active: true,
        })
    }

    /// Look up an active rift by project and name, returning its id, owner, and privacy flag
    pub async fn find_rift_access_info(
        &self,
        project_id: ProjectId,
        name: &str,
    ) -> Result<Option<(RiftId, Option<UserId>, bool)>> {
        let row = sqlx::query_as::<_, (Uuid, Option<Uuid>, bool)>(
            "SELECT id, created_by, is_private FROM rifts WHERE project_id = $1 AND name = $2 AND is_active = true",
        )
        .bind(project_id)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Ids of private rifts in a project, used to hide them from non-collaborators
    pub async fn private_rift_ids_for_project(&self, project_id: ProjectId) -> Result<Vec<RiftId>> {
        let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM rifts WHERE project_id = $1 AND is_private = true")
            .bind(project_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(ids)
    }

    /// Whether the user is a collaborator on the rift
    pub async fn is_rift_collaborator(&self, rift_id: RiftId, user_id: UserId) -> Result<bool> {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM rift_collaborators WHERE rift_id = $1 AND user_id = $2)",
        )
        .bind(rift_id)
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(exists)
    }

    /// Record a pending request to join a rift; repeated requests are a no-op
    pub async fn create_rift_access_request(&self, rift_id: RiftId, user_id: UserId) -> Result<()> {
        sqlx::query(
            "INSERT INTO rift_access_requests (rift_id, user_id) VALUES ($1, $2) ON CONFLICT (rift_id, user_id) DO NOTHING",
        )
        .bind(rift_id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Pending access requests for a rift as (user id, username, requested at)
    pub async fn list_rift_access_requests(
        &self,
        rift_id: RiftId,
    ) -> Result<Vec<(UserId, String, DateTime<Utc>)>> {
        let rows = sqlx::query_as::<_, (Uuid, String, DateTime<Utc>)>(
            "SELECT u.id, u.username, r.created_at FROM rift_access_requests r \
             JOIN users u ON u.id = r.user_id WHERE r.rift_id = $1 ORDER BY r.created_at",
        )
        .bind(rift_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Add a user to a rift and to its project, clearing any pending request
    pub async fn add_rift_member(&self, project_id: ProjectId, rift_id: RiftId, user_id: UserId) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO project_members (project_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(project_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO rift_collaborators (rift_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(rift_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM rift_access_requests WHERE rift_id = $1 AND user_id = $2")
            .bind(rift_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Remove a user from a rift's collaborators; project membership is left unchanged
    pub async fn remove_rift_member(&self, rift_id: RiftId, user_id: UserId) -> Result<()> {
        sqlx::query("DELETE FROM rift_collaborators WHERE rift_id = $1 AND user_id = $2")
            .bind(rift_id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Whether an active wormhole with this name already exists in the project
    pub async fn active_wormhole_name_exists(&self, project_id: ProjectId, name: &str) -> Result<bool> {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM wormholes WHERE project_id = $1 AND name = $2 AND status = 'active')",
        )
        .bind(project_id)
        .bind(name)
        .fetch_one(&self.pool)
        .await?;
        Ok(exists)
    }

    /// Insert a wormhole with its source rifts and base snapshot in one transaction
    pub async fn create_wormhole(
        &self,
        project_id: ProjectId,
        name: &str,
        created_by: UserId,
        base_hashes: &HashMap<String, String>,
        rift_ids: &[RiftId],
        is_private: bool,
    ) -> Result<WormholeRecord> {
        let id = Uuid::new_v4();
        let base_json = serde_json::to_string(base_hashes)?;
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            "INSERT INTO wormholes (id, project_id, name, created_by, base_files, is_private) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(id)
        .bind(project_id)
        .bind(name)
        .bind(created_by)
        .bind(base_json)
        .bind(is_private)
        .execute(&mut *tx)
        .await?;

        for rift_id in rift_ids {
            sqlx::query("INSERT INTO wormhole_rifts (wormhole_id, rift_id) VALUES ($1, $2)")
                .bind(id)
                .bind(rift_id)
                .execute(&mut *tx)
                .await?;
        }

        tx.commit().await?;
        self.get_wormhole(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("wormhole {} missing after insert", id))
    }

    /// Load one wormhole by id
    pub async fn get_wormhole(&self, wormhole_id: Uuid) -> Result<Option<WormholeRecord>> {
        let rows: Vec<WormholeRow> = sqlx::query_as(&format!(
            "SELECT {WORMHOLE_COLUMNS} FROM wormholes WHERE id = $1"
        ))
        .bind(wormhole_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(self.hydrate_wormholes(rows).await?.into_iter().next())
    }

    /// List every wormhole in a project, newest first
    pub async fn list_project_wormholes(&self, project_id: ProjectId) -> Result<Vec<WormholeRecord>> {
        let rows: Vec<WormholeRow> = sqlx::query_as(&format!(
            "SELECT {WORMHOLE_COLUMNS} FROM wormholes WHERE project_id = $1 ORDER BY created_at DESC"
        ))
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        self.hydrate_wormholes(rows).await
    }

    /// IDs of active wormholes that include the given rift as one of their sides
    pub async fn active_wormholes_for_rift(&self, rift_id: RiftId) -> Result<Vec<Uuid>> {
        let rows: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT w.id FROM wormholes w \
             JOIN wormhole_rifts wr ON wr.wormhole_id = w.id \
             WHERE wr.rift_id = $1 AND w.status = 'active'",
        )
        .bind(rift_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    async fn hydrate_wormholes(&self, rows: Vec<WormholeRow>) -> Result<Vec<WormholeRecord>> {
        let mut records = Vec::with_capacity(rows.len());
        for (id, project_id, name, status, created_by, created_at, converted_rift_id, base_files, is_private) in rows {
            let rift_rows: Vec<(Uuid, bool)> = sqlx::query_as(
                "SELECT rift_id, enabled FROM wormhole_rifts WHERE wormhole_id = $1 ORDER BY rift_id",
            )
            .bind(id)
            .fetch_all(&self.pool)
            .await?;
            let mut rift_ids = Vec::with_capacity(rift_rows.len());
            let mut disabled_rift_ids = Vec::new();
            for (rift_id, enabled) in rift_rows {
                rift_ids.push(rift_id);
                if !enabled {
                    disabled_rift_ids.push(rift_id);
                }
            }
            records.push(WormholeRecord {
                id,
                project_id,
                name,
                status,
                created_by,
                created_at,
                converted_rift_id,
                base_hashes: serde_json::from_str(&base_files)?,
                rift_ids,
                disabled_rift_ids,
                is_private,
            });
        }
        Ok(records)
    }

    /// Toggles or sets whether a rift is enabled in a wormhole. Returns `Some(new_enabled)` if the rift belongs to the wormhole.
    pub async fn set_wormhole_rift_enabled(
        &self,
        wormhole_id: Uuid,
        rift_id: RiftId,
        enabled: Option<bool>,
    ) -> Result<Option<bool>> {
        let row: Option<(bool,)> = match enabled {
            Some(val) => {
                sqlx::query_as(
                    "UPDATE wormhole_rifts SET enabled = $3 WHERE wormhole_id = $1 AND rift_id = $2 RETURNING enabled",
                )
                .bind(wormhole_id)
                .bind(rift_id)
                .bind(val)
                .fetch_optional(&self.pool)
                .await?
            }
            None => {
                sqlx::query_as(
                    "UPDATE wormhole_rifts SET enabled = NOT enabled WHERE wormhole_id = $1 AND rift_id = $2 RETURNING enabled",
                )
                .bind(wormhole_id)
                .bind(rift_id)
                .fetch_optional(&self.pool)
                .await?
            }
        };
        Ok(row.map(|(e,)| e))
    }

    /// Permanently delete a wormhole and its cascaded rows
    pub async fn delete_wormhole(&self, wormhole_id: Uuid) -> Result<bool> {
        let res = sqlx::query("DELETE FROM wormholes WHERE id = $1")
            .bind(wormhole_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Change a wormhole's lifecycle status, optionally recording the rift it converged into
    pub async fn set_wormhole_status(
        &self,
        wormhole_id: Uuid,
        status: &str,
        converted_rift_id: Option<RiftId>,
    ) -> Result<()> {
        sqlx::query("UPDATE wormholes SET status = $2, converted_rift_id = $3 WHERE id = $1")
            .bind(wormhole_id)
            .bind(status)
            .bind(converted_rift_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Stored resolutions for a wormhole, keyed by file path then conflict id
    pub async fn get_wormhole_resolutions(
        &self,
        wormhole_id: Uuid,
    ) -> Result<HashMap<String, HashMap<String, Resolution>>> {
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT path, conflict_id, resolution FROM wormhole_resolutions WHERE wormhole_id = $1",
        )
        .bind(wormhole_id)
        .fetch_all(&self.pool)
        .await?;

        let mut by_path: HashMap<String, HashMap<String, Resolution>> = HashMap::new();
        for (path, conflict_id, resolution) in rows {
            by_path
                .entry(path)
                .or_default()
                .insert(conflict_id, serde_json::from_str(&resolution)?);
        }
        Ok(by_path)
    }

    /// Record or replace the resolution for one conflict
    pub async fn upsert_wormhole_resolution(
        &self,
        wormhole_id: Uuid,
        path: &str,
        conflict_id: &str,
        resolution: &Resolution,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO wormhole_resolutions (wormhole_id, path, conflict_id, resolution) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (wormhole_id, path, conflict_id) DO UPDATE SET resolution = EXCLUDED.resolution",
        )
        .bind(wormhole_id)
        .bind(path)
        .bind(conflict_id)
        .bind(serde_json::to_string(resolution)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Get a rift by ID
    pub async fn get_rift(&self, rift_id: RiftId) -> Result<Option<Rift>> {
        let rift = sqlx::query!(
            r#"
            SELECT r.id, r.project_id, r.name, r.parent_rift_id, r.created_at, r.is_active,
                   ARRAY_AGG(rc.user_id) as collaborators
            FROM rifts r
            LEFT JOIN rift_collaborators rc ON r.id = rc.rift_id
            WHERE r.id = $1
            GROUP BY r.id, r.project_id, r.name, r.parent_rift_id, r.created_at, r.is_active
            "#,
            rift_id
        )
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = rift {
            let collaborators = row.collaborators
                .unwrap_or_default();

            Ok(Some(Rift {
                id: row.id,
                project_id: row.project_id,
                name: row.name,
                parent_rift: row.parent_rift_id,
                collaborators,
                created_at: row.created_at,
                last_checkpoint: None, // TODO: Get from checkpoints
                is_active: row.is_active,
            }))
        } else {
            Ok(None)
        }
    }

    /// Get user's primary rift for a project
    pub async fn get_user_rift(&self, project_id: ProjectId, user_id: UserId) -> Result<Option<Rift>> {
        // First, check if user has an existing rift for this project
        let rift = sqlx::query!(
            r#"
            SELECT r.id, r.project_id, r.name, r.parent_rift_id, r.created_at, r.is_active
            FROM rifts r
            INNER JOIN rift_collaborators rc ON r.id = rc.rift_id
            WHERE r.project_id = $1 AND rc.user_id = $2 AND r.is_active = true
            ORDER BY r.created_at ASC
            LIMIT 1
            "#,
            project_id,
            user_id
        )
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = rift {
            Ok(Some(Rift {
                id: row.id,
                project_id: row.project_id,
                name: row.name,
                parent_rift: row.parent_rift_id,
                collaborators: vec![user_id], // Simplified for now
                created_at: row.created_at,
                last_checkpoint: None, // TODO: Get from checkpoints
                is_active: row.is_active,
            }))
        } else {
            Ok(None)
        }
    }

    /// Find a rift by exact name that the user collaborates on in a project
    pub async fn get_user_rift_by_name(&self, project_id: ProjectId, user_id: UserId, name: &str) -> Result<Option<Rift>> {
        let rift_id: Option<Uuid> = sqlx::query_scalar(
            r#"
            SELECT r.id
            FROM rifts r
            INNER JOIN rift_collaborators rc ON r.id = rc.rift_id
            WHERE r.project_id = $1 AND rc.user_id = $2 AND r.name = $3 AND r.is_active = true
            ORDER BY r.created_at ASC
            LIMIT 1
            "#,
        )
        .bind(project_id)
        .bind(user_id)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;

        match rift_id {
            Some(id) => self.get_rift(id).await,
            None => Ok(None),
        }
    }

    /// Check if user has access to a project
    pub async fn user_has_project_access(&self, user_id: UserId, project_id: ProjectId) -> Result<bool> {
        let count = sqlx::query!(
            "SELECT COUNT(*) as count FROM project_members WHERE user_id = $1 AND project_id = $2",
            user_id,
            project_id
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(count.count.unwrap_or(0) > 0)
    }

    /// Returns whether a project is marked private. Missing projects are reported as not private.
    pub async fn is_project_private(&self, project_id: ProjectId) -> Result<bool> {
        let private: Option<bool> = sqlx::query_scalar("SELECT is_private FROM projects WHERE id = $1")
            .bind(project_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(private.unwrap_or(false))
    }

    pub async fn create_user(&self, username: String, email: String, role: UserRole) -> Result<User> {
        let user_id = Uuid::new_v4();
        self.create_user_with_id(user_id, username, email, role).await
    }

    /// Create a new user with a specific user ID (for OAuth user recreation)
    pub async fn create_user_with_id(&self, user_id: UserId, username: String, email: String, role: UserRole) -> Result<User> {
        tracing::info!("Creating user with ID: {}, username: {}, email: {}", user_id, username, email);
        
        // First try to find existing user by email or username
        if let Some(existing_user) = self.get_user_by_email(&email).await? {
            tracing::info!("Found existing user by email during create_user_with_id: {} ({})", existing_user.username, existing_user.id);
            return Ok(existing_user);
        }
        
        if let Some(existing_user) = self.get_user_by_username(&username).await? {
            tracing::info!("Found existing user by username during create_user_with_id: {} ({})", existing_user.username, existing_user.id);
            return Ok(existing_user);
        }

        let user = sqlx::query!(
            r#"
            INSERT INTO users (id, username, email, role)
            VALUES ($1, $2, $3, $4)
            RETURNING id, username, email, role as "role: UserRole", created_at
            "#,
            user_id,
            username,
            email,
            role as UserRole
        )
        .fetch_one(&self.pool)
        .await?;

        tracing::info!("Successfully created/updated user: {} ({})", user.username, user.id);

        Ok(User {
            id: user.id,
            username: user.username,
            email: user.email,
            role: user.role,
            created_at: user.created_at,
        })
    }

    /// Check if user exists by email
    pub async fn user_exists_by_email(&self, email: &str) -> Result<bool> {
        let count = sqlx::query!(
            "SELECT COUNT(*) as count FROM users WHERE email = $1",
            email
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(count.count.unwrap_or(0) > 0)
    }

    /// Check if user exists by username
    pub async fn user_exists_by_username(&self, username: &str) -> Result<bool> {
        let count = sqlx::query!(
            "SELECT COUNT(*) as count FROM users WHERE username = $1",
            username
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(count.count.unwrap_or(0) > 0)
    }

    /// Check if user has admin permissions
    pub async fn user_is_admin(&self, user_id: UserId) -> Result<bool> {
        let user = sqlx::query!(
            "SELECT role as \"role: UserRole\" FROM users WHERE id = $1",
            user_id
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(user.map_or(false, |u| matches!(u.role, UserRole::Admin | UserRole::SuperAdmin)))
    }

    /// Check if user has super admin permissions
    pub async fn user_is_super_admin(&self, user_id: UserId) -> Result<bool> {
        let user = sqlx::query!(
            "SELECT role as \"role: UserRole\" FROM users WHERE id = $1",
            user_id
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(user.map_or(false, |u| u.role == UserRole::SuperAdmin))
    }

    /// Check if project exists by name
    pub async fn project_exists_by_name(&self, name: &str) -> Result<bool> {
        let count = sqlx::query!(
            "SELECT COUNT(*) as count FROM projects WHERE name = $1",
            name
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(count.count.unwrap_or(0) > 0)
    }

    /// Create a new project
    pub async fn create_project(&self, name: String, description: String, members: Vec<UserId>) -> Result<Project> {
        self.create_project_with_privacy(name, description, members, false).await
    }

    /// Create a project; private projects are only listed to their members
    pub async fn create_project_with_privacy(
        &self,
        name: String,
        description: String,
        members: Vec<UserId>,
        is_private: bool,
    ) -> Result<Project> {
        let project_id = Uuid::new_v4();
        
        // Start transaction
        let mut tx = self.pool.begin().await?;

        // Create the project
        let project = sqlx::query_as::<_, (Uuid, String, Option<String>, DateTime<Utc>)>(
            "INSERT INTO projects (id, name, description, is_private) VALUES ($1, $2, $3, $4) \
             RETURNING id, name, description, created_at",
        )
        .bind(project_id)
        .bind(&name)
        .bind(&description)
        .bind(is_private)
        .fetch_one(&mut *tx)
        .await?;

        // Add project members
        for member_id in &members {
            sqlx::query!(
                "INSERT INTO project_members (project_id, user_id) VALUES ($1, $2)",
                project_id,
                member_id
            )
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;

        Ok(Project {
            id: project.0,
            name: project.1,
            description: project.2.unwrap_or_default(),
            members,
            created_at: project.3,
            settings: ProjectSettings::default(),
        })
    }

    /// Get project by name
    pub async fn get_project_by_name(&self, name: &str) -> Result<Option<Project>> {
        let project_row = sqlx::query!(
            "SELECT id, name, description, created_at FROM projects WHERE name = $1",
            name
        )
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = project_row {
            // Get project members
            let members = sqlx::query!(
                "SELECT user_id FROM project_members WHERE project_id = $1",
                row.id
            )
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|row| row.user_id)
            .collect();

            Ok(Some(Project {
                id: row.id,
                name: row.name,
                description: row.description.unwrap_or_default(),
                members,
                created_at: row.created_at,
                settings: ProjectSettings::default(),
            }))
        } else {
            Ok(None)
        }
    }

    /// Delete a project and all associated data
    pub async fn delete_project(&self, project_id: ProjectId) -> Result<()> {
        // PostgreSQL will handle cascading deletes for:
        // - project_members (ON DELETE CASCADE)
        // - rifts (ON DELETE CASCADE) 
        // - rift_collaborators (through rifts CASCADE)
        // - project_settings (ON DELETE CASCADE)
        
        let result = sqlx::query!(
            "DELETE FROM projects WHERE id = $1",
            project_id
        )
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(anyhow::anyhow!("Project not found or already deleted"));
        }

        tracing::info!("Successfully deleted project {} and all associated data", project_id);
        Ok(())
    }
} 