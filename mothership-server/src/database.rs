use anyhow::Result;
use chrono::Utc;
use mothership_common::{
    GatewayProject, Project, ProjectId, ProjectSettings, Rift, RiftId, User, UserId, UserRole,
};
use sqlx::PgPool;
use uuid::Uuid;

/// PostgreSQL database implementation
#[derive(Clone)]
pub struct Database {
    pool: PgPool,
}

impl Database {
    /// Create a new database connection pool
    pub async fn new(database_url: &str) -> Result<Self> {
        tracing::info!("🔗 Connecting to PostgreSQL database...");
        
        let pool = PgPool::connect(database_url).await?;
        
        tracing::info!("✅ Successfully connected to PostgreSQL database");
        
        Ok(Self { pool })
    }

    /// Run database migrations manually (since we can't use sqlx migrate in Docker build)
    pub async fn ensure_schema(&self) -> Result<()> {
        tracing::info!("🔄 Ensuring database schema exists...");
        
        // Create the schema manually using runtime queries
        sqlx::query("CREATE EXTENSION IF NOT EXISTS \"uuid-ossp\"")
            .execute(&self.pool).await?;
            
        // Create user_role enum type (ignore error if it already exists)
        let _ = sqlx::query("CREATE TYPE user_role AS ENUM ('user', 'admin', 'super_admin')")
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
        
        tracing::info!("✅ Database schema ready!");
        Ok(())
    }

    /// Get all projects a user has access to
    pub async fn get_user_projects(
        &self,
        user_id: UserId,
        _include_inactive: bool,
    ) -> Result<Vec<GatewayProject>> {
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

        let mut gateway_projects = Vec::new();

        for project_row in projects {
            let project = Project {
                id: project_row.id,
                name: project_row.name,
                description: project_row.description.unwrap_or_default(),
                members: vec![user_id], // Simplified for now
                created_at: project_row.created_at,
                settings: ProjectSettings::default(),
            };

            // For now, return empty rifts - we'll implement this next
            gateway_projects.push(GatewayProject {
                project,
                active_rifts: vec![],
                your_rifts: vec![],
                last_activity: None,
            });
        }

        Ok(gateway_projects)
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

    /// Create a new rift for a user in a project
    pub async fn create_rift(
        &self,
        project_id: ProjectId,
        user_id: UserId,
        rift_name: Option<String>,
    ) -> Result<Rift> {
        let rift_id = Uuid::new_v4();
        let name = rift_name.unwrap_or_else(|| "main".to_string());
        
        // Create the rift
        sqlx::query!(
            r#"
            INSERT INTO rifts (id, project_id, name, is_active)
            VALUES ($1, $2, $3, true)
            "#,
            rift_id,
            project_id,
            name
        )
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

    /// Create a new user with specified role
    pub async fn create_user(&self, username: String, email: String, role: UserRole) -> Result<User> {
        let user_id = Uuid::new_v4();
        self.create_user_with_id(user_id, username, email, role).await
    }

    /// Create a new user with a specific user ID (for OAuth user recreation)
    pub async fn create_user_with_id(&self, user_id: UserId, username: String, email: String, role: UserRole) -> Result<User> {
        tracing::info!("🔄 Creating user with ID: {}, username: {}, email: {}", user_id, username, email);
        
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

        tracing::info!("✅ Successfully created/updated user: {} ({})", user.username, user.id);

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
        let project_id = Uuid::new_v4();
        
        // Start transaction
        let mut tx = self.pool.begin().await?;

        // Create the project
        let project = sqlx::query!(
            r#"
            INSERT INTO projects (id, name, description)
            VALUES ($1, $2, $3)
            RETURNING id, name, description, created_at
            "#,
            project_id,
            name,
            description
        )
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
            id: project.id,
            name: project.name,
            description: project.description.unwrap_or_default(),
            members,
            created_at: project.created_at,
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