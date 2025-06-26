use anyhow::Result;
use chrono::Utc;
use mothership_common::{
    GatewayProject, Project, ProjectId, ProjectSettings, Rift, RiftId, RiftSummary, User, UserId, UserRole,
};
use std::collections::HashMap;
use tokio::sync::RwLock;
use uuid::Uuid;

/// In-memory database for development/testing
/// In production, this would be backed by PostgreSQL
#[derive(Clone)]
pub struct Database {
    users: std::sync::Arc<RwLock<HashMap<UserId, User>>>,
    projects: std::sync::Arc<RwLock<HashMap<ProjectId, Project>>>,
    rifts: std::sync::Arc<RwLock<HashMap<RiftId, Rift>>>,
    project_members: std::sync::Arc<RwLock<HashMap<ProjectId, Vec<UserId>>>>,
}

impl Database {
    pub async fn new() -> Result<Self> {
        let db = Self {
            users: std::sync::Arc::new(RwLock::new(HashMap::new())),
            projects: std::sync::Arc::new(RwLock::new(HashMap::new())),
            rifts: std::sync::Arc::new(RwLock::new(HashMap::new())),
            project_members: std::sync::Arc::new(RwLock::new(HashMap::new())),
        };

        // Initialize with some demo data
        db.init_demo_data().await?;

        Ok(db)
    }

    /// Initialize some demo data for testing
    async fn init_demo_data(&self) -> Result<()> {
        // Create demo users
        let alice_id = Uuid::new_v4();
        let bob_id = Uuid::new_v4();

        let alice = User {
            id: alice_id,
            username: "alice".to_string(),
            email: "alice@example.com".to_string(),
            role: UserRole::Admin,  // Alice is an admin
            created_at: Utc::now(),
        };

        let bob = User {
            id: bob_id,
            username: "bob".to_string(),
            email: "bob@example.com".to_string(),
            role: UserRole::User,   // Bob is a regular user
            created_at: Utc::now(),
        };

        // Store users
        {
            let mut users = self.users.write().await;
            users.insert(alice_id, alice);
            users.insert(bob_id, bob);
        }

        // Create demo projects
        let project1_id = Uuid::new_v4();
        let project2_id = Uuid::new_v4();

        let project1 = Project {
            id: project1_id,
            name: "Mothership Core".to_string(),
            description: "The main Mothership project".to_string(),
            members: vec![alice_id, bob_id],
            created_at: Utc::now(),
            settings: ProjectSettings::default(),
        };

        let project2 = Project {
            id: project2_id,
            name: "Demo App".to_string(),
            description: "A demo application for testing".to_string(),
            members: vec![alice_id],
            created_at: Utc::now(),
            settings: ProjectSettings::default(),
        };

        // Store projects
        {
            let mut projects = self.projects.write().await;
            projects.insert(project1_id, project1);
            projects.insert(project2_id, project2);
        }

        // Store project memberships
        {
            let mut members = self.project_members.write().await;
            members.insert(project1_id, vec![alice_id, bob_id]);
            members.insert(project2_id, vec![alice_id]);
        }

        // Create some demo rifts
        let alice_rift_id = Uuid::new_v4();
        let bob_rift_id = Uuid::new_v4();

        let alice_rift = Rift {
            id: alice_rift_id,
            project_id: project1_id,
            name: "alice/main".to_string(),
            parent_rift: None,
            collaborators: vec![alice_id],
            created_at: Utc::now(),
            last_checkpoint: None,
            is_active: true,
        };

        let bob_rift = Rift {
            id: bob_rift_id,
            project_id: project1_id,
            name: "bob/feature-auth".to_string(),
            parent_rift: Some(alice_rift_id),
            collaborators: vec![bob_id],
            created_at: Utc::now(),
            last_checkpoint: None,
            is_active: true,
        };

        // Store rifts
        {
            let mut rifts = self.rifts.write().await;
            rifts.insert(alice_rift_id, alice_rift);
            rifts.insert(bob_rift_id, bob_rift);
        }

        Ok(())
    }

    /// Get all projects a user has access to
    pub async fn get_user_projects(
        &self,
        user_id: UserId,
        include_inactive: bool,
    ) -> Result<Vec<GatewayProject>> {
        let projects = self.projects.read().await;
        let rifts = self.rifts.read().await;
        let users = self.users.read().await;

        let mut gateway_projects = Vec::new();

        for project in projects.values() {
            // Check if user is a member
            if !project.members.contains(&user_id) {
                continue;
            }

            // Get rifts for this project
            let project_rifts: Vec<&Rift> = rifts
                .values()
                .filter(|r| r.project_id == project.id)
                .filter(|r| include_inactive || r.is_active)
                .collect();

            // Separate user's rifts from others
            let your_rifts: Vec<RiftSummary> = project_rifts
                .iter()
                .filter(|r| r.collaborators.contains(&user_id))
                .map(|r| self.rift_to_summary(r, &users))
                .collect();

            let active_rifts: Vec<RiftSummary> = project_rifts
                .iter()
                .filter(|r| !r.collaborators.contains(&user_id))
                .map(|r| self.rift_to_summary(r, &users))
                .collect();

            let last_activity = project_rifts
                .iter()
                .filter_map(|r| r.last_checkpoint)
                .map(|_| Utc::now()) // Simplified - would get actual checkpoint time
                .max();

            gateway_projects.push(GatewayProject {
                project: project.clone(),
                active_rifts,
                your_rifts,
                last_activity,
            });
        }

        Ok(gateway_projects)
    }

    /// Convert rift to summary for display
    fn rift_to_summary(&self, rift: &Rift, users: &HashMap<UserId, User>) -> RiftSummary {
        let collaborators: Vec<String> = rift
            .collaborators
            .iter()
            .filter_map(|id| users.get(id))
            .map(|u| u.username.clone())
            .collect();

        RiftSummary {
            id: rift.id,
            name: rift.name.clone(),
            collaborators,
            last_checkpoint: None, // Simplified
            change_count: 0,       // Simplified
        }
    }

    /// Get a specific project
    pub async fn get_project(&self, project_id: ProjectId) -> Result<Option<Project>> {
        let projects = self.projects.read().await;
        Ok(projects.get(&project_id).cloned())
    }

    /// List all projects (for testing)
    pub async fn list_all_projects(&self) -> Result<Vec<Project>> {
        let projects = self.projects.read().await;
        Ok(projects.values().cloned().collect())
    }

    /// Get user by ID
    pub async fn get_user(&self, user_id: UserId) -> Result<Option<User>> {
        let users = self.users.read().await;
        Ok(users.get(&user_id).cloned())
    }

    /// Get user by username
    pub async fn get_user_by_username(&self, username: &str) -> Result<Option<User>> {
        let users = self.users.read().await;
        Ok(users
            .values()
            .find(|u| u.username == username)
            .cloned())
    }

    /// Get user by email
    pub async fn get_user_by_email(&self, email: &str) -> Result<Option<User>> {
        let users = self.users.read().await;
        Ok(users
            .values()
            .find(|u| u.email == email)
            .cloned())
    }

    /// Create a new rift for a user in a project
    pub async fn create_rift(
        &self,
        project_id: ProjectId,
        user_id: UserId,
        rift_name: Option<String>,
    ) -> Result<Rift> {
        let rift_id = Uuid::new_v4();
        
        // Generate rift name if not provided
        let name = if let Some(name) = rift_name {
            name
        } else {
            let users = self.users.read().await;
            if let Some(user) = users.get(&user_id) {
                format!("{}/main", user.username)
            } else {
                format!("user-{}/main", user_id)
            }
        };

        let rift = Rift {
            id: rift_id,
            project_id,
            name,
            parent_rift: None, // For now, all rifts are top-level
            collaborators: vec![user_id],
            created_at: Utc::now(),
            last_checkpoint: None,
            is_active: true,
        };

        // Store the rift
        {
            let mut rifts = self.rifts.write().await;
            rifts.insert(rift_id, rift.clone());
        }

        Ok(rift)
    }

    /// Get a rift by ID
    pub async fn get_rift(&self, rift_id: RiftId) -> Result<Option<Rift>> {
        let rifts = self.rifts.read().await;
        Ok(rifts.get(&rift_id).cloned())
    }

    /// Check if user has access to a project
    pub async fn user_has_project_access(&self, user_id: UserId, project_id: ProjectId) -> Result<bool> {
        let projects = self.projects.read().await;
        if let Some(project) = projects.get(&project_id) {
            Ok(project.members.contains(&user_id))
        } else {
            Ok(false)
        }
    }

    /// Create a new user with specified role
    pub async fn create_user(&self, username: String, email: String, role: UserRole) -> Result<User> {
        let user_id = Uuid::new_v4();
        
        let user = User {
            id: user_id,
            username,
            email,
            role,
            created_at: Utc::now(),
        };

        // Store the user
        {
            let mut users = self.users.write().await;
            users.insert(user_id, user.clone());
        }

        Ok(user)
    }

    /// Check if user exists by email
    pub async fn user_exists_by_email(&self, email: &str) -> Result<bool> {
        let users = self.users.read().await;
        Ok(users.values().any(|u| u.email == email))
    }

    /// Check if user exists by username
    pub async fn user_exists_by_username(&self, username: &str) -> Result<bool> {
        let users = self.users.read().await;
        Ok(users.values().any(|u| u.username == username))
    }

    /// Check if user has admin permissions
    pub async fn user_is_admin(&self, user_id: UserId) -> Result<bool> {
        let users = self.users.read().await;
        if let Some(user) = users.get(&user_id) {
            Ok(matches!(user.role, UserRole::Admin | UserRole::SuperAdmin))
        } else {
            Ok(false)
        }
    }

    /// Check if user has super admin permissions
    pub async fn user_is_super_admin(&self, user_id: UserId) -> Result<bool> {
        let users = self.users.read().await;
        if let Some(user) = users.get(&user_id) {
            Ok(user.role == UserRole::SuperAdmin)
        } else {
            Ok(false)
        }
    }

    /// Check if project exists by name
    pub async fn project_exists_by_name(&self, name: &str) -> Result<bool> {
        let projects = self.projects.read().await;
        Ok(projects.values().any(|p| p.name == name))
    }

    /// Create a new project
    pub async fn create_project(&self, name: String, description: String, members: Vec<UserId>) -> Result<Project> {
        let project_id = Uuid::new_v4();
        
        let project = Project {
            id: project_id,
            name,
            description,
            members: members.clone(),
            created_at: Utc::now(),
            settings: ProjectSettings::default(),
        };

        // Store the project
        {
            let mut projects = self.projects.write().await;
            projects.insert(project_id, project.clone());
        }

        // Store project membership
        {
            let mut project_members = self.project_members.write().await;
            project_members.insert(project_id, members);
        }

        Ok(project)
    }

    /// Get project by name
    pub async fn get_project_by_name(&self, name: &str) -> Result<Option<Project>> {
        let projects = self.projects.read().await;
        Ok(projects.values().find(|p| p.name == name).cloned())
    }
} 