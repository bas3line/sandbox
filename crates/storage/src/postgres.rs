use async_trait::async_trait;
use chrono::{Duration, Utc};
use diesel::{BoolExpressionMethods, ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::{
    AsyncPgConnection, RunQueryDsl, SimpleAsyncConnection,
    pooled_connection::{AsyncDieselConnectionManager, bb8::Pool},
};
use sandbox_core::{
    NodeId, OperationId, SandboxId,
    api::{CompleteAssignmentRequest, HeartbeatRequest},
    model::{Assignment, AssignmentState, NodeRecord, Operation, OperationState, Sandbox},
};
use serde_json::Value;
use uuid::Uuid;

use crate::{Store, StoreError, StoreResult};

diesel::table! {
    nodes (id) {
        id -> Uuid,
        record -> Jsonb,
        updated_at -> Timestamptz,
    }
}

diesel::table! {
    sandboxes (id) {
        id -> Uuid,
        tenant -> Text,
        record -> Jsonb,
        updated_at -> Timestamptz,
    }
}

diesel::table! {
    operations (id) {
        id -> Uuid,
        sandbox_id -> Uuid,
        record -> Jsonb,
        updated_at -> Timestamptz,
    }
}

diesel::table! {
    assignments (id) {
        id -> Uuid,
        node_id -> Uuid,
        operation_id -> Uuid,
        sandbox_id -> Uuid,
        state -> Text,
        lease_until -> Nullable<Timestamptz>,
        record -> Jsonb,
        created_at -> Timestamptz,
    }
}

#[derive(diesel::Insertable)]
#[diesel(table_name = nodes)]
struct NewNodeRow {
    id: Uuid,
    record: Value,
    updated_at: chrono::DateTime<Utc>,
}

#[derive(diesel::Insertable)]
#[diesel(table_name = sandboxes)]
struct NewSandboxRow {
    id: Uuid,
    tenant: String,
    record: Value,
    updated_at: chrono::DateTime<Utc>,
}

#[derive(diesel::Insertable)]
#[diesel(table_name = operations)]
struct NewOperationRow {
    id: Uuid,
    sandbox_id: Uuid,
    record: Value,
    updated_at: chrono::DateTime<Utc>,
}

#[derive(diesel::Insertable)]
#[diesel(table_name = assignments)]
struct NewAssignmentRow {
    id: Uuid,
    node_id: Uuid,
    operation_id: Uuid,
    sandbox_id: Uuid,
    state: String,
    lease_until: Option<chrono::DateTime<Utc>>,
    record: Value,
    created_at: chrono::DateTime<Utc>,
}

pub struct PgStore {
    pool: Pool<AsyncPgConnection>,
}

impl PgStore {
    pub async fn connect(database_url: &str, max_connections: u32) -> StoreResult<Self> {
        let manager = AsyncDieselConnectionManager::<AsyncPgConnection>::new(database_url);
        let pool = Pool::builder()
            .max_size(max_connections.max(1))
            .build(manager)
            .await
            .map_err(backend)?;
        Ok(Self { pool })
    }

    pub async fn migrate(&self) -> StoreResult<()> {
        let mut connection = self.pool.get().await.map_err(backend)?;
        connection
            .batch_execute(include_str!("../migrations/00000000000000_init/up.sql"))
            .await
            .map_err(backend)
    }

    async fn put_operation(&self, operation: &Operation) -> StoreResult<()> {
        let mut connection = self.pool.get().await.map_err(backend)?;
        let row = NewOperationRow {
            id: operation.id.0,
            sandbox_id: operation.sandbox_id.0,
            record: serde_json::to_value(operation)?,
            updated_at: operation.updated_at,
        };
        diesel::insert_into(operations::table)
            .values(&row)
            .on_conflict(operations::id)
            .do_update()
            .set((
                operations::record.eq(&row.record),
                operations::updated_at.eq(row.updated_at),
            ))
            .execute(&mut connection)
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn put_sandbox(&self, sandbox: &Sandbox) -> StoreResult<()> {
        let mut connection = self.pool.get().await.map_err(backend)?;
        let row = NewSandboxRow {
            id: sandbox.id.0,
            tenant: sandbox.spec.tenant.clone(),
            record: serde_json::to_value(sandbox)?,
            updated_at: sandbox.updated_at,
        };
        diesel::insert_into(sandboxes::table)
            .values(&row)
            .on_conflict(sandboxes::id)
            .do_update()
            .set((
                sandboxes::record.eq(&row.record),
                sandboxes::updated_at.eq(row.updated_at),
            ))
            .execute(&mut connection)
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn put_assignment(&self, assignment: &Assignment) -> StoreResult<()> {
        let mut connection = self.pool.get().await.map_err(backend)?;
        let row = NewAssignmentRow {
            id: assignment.id.0,
            node_id: assignment.node_id.0,
            operation_id: assignment.operation_id.0,
            sandbox_id: assignment.sandbox_id.0,
            state: state_name(assignment.state).into(),
            lease_until: assignment.lease_until,
            record: serde_json::to_value(assignment)?,
            created_at: assignment.created_at,
        };
        diesel::insert_into(assignments::table)
            .values(&row)
            .on_conflict(assignments::id)
            .do_update()
            .set((
                assignments::state.eq(&row.state),
                assignments::lease_until.eq(row.lease_until),
                assignments::record.eq(&row.record),
            ))
            .execute(&mut connection)
            .await
            .map_err(backend)?;
        Ok(())
    }
}

#[async_trait]
impl Store for PgStore {
    fn backend_name(&self) -> &'static str {
        "postgres"
    }

    async fn upsert_node(&self, node: NodeRecord) -> StoreResult<()> {
        let mut connection = self.pool.get().await.map_err(backend)?;
        let row = NewNodeRow {
            id: node.id.0,
            record: serde_json::to_value(&node)?,
            updated_at: Utc::now(),
        };
        diesel::insert_into(nodes::table)
            .values(&row)
            .on_conflict(nodes::id)
            .do_update()
            .set((
                nodes::record.eq(&row.record),
                nodes::updated_at.eq(row.updated_at),
            ))
            .execute(&mut connection)
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn heartbeat_node(&self, id: NodeId, heartbeat: HeartbeatRequest) -> StoreResult<()> {
        let mut node = self
            .list_nodes()
            .await?
            .into_iter()
            .find(|node| node.id == id)
            .ok_or_else(|| StoreError::NotFound(format!("node {id}")))?;
        node.capacity = heartbeat.capacity;
        node.pressure = heartbeat.pressure;
        node.warm_images = heartbeat.warm_images;
        node.last_seen = Utc::now();
        self.upsert_node(node).await
    }

    async fn list_nodes(&self) -> StoreResult<Vec<NodeRecord>> {
        let mut connection = self.pool.get().await.map_err(backend)?;
        let rows = nodes::table
            .select(nodes::record)
            .load::<Value>(&mut connection)
            .await
            .map_err(backend)?;
        rows.into_iter()
            .map(|record| serde_json::from_value(record).map_err(Into::into))
            .collect()
    }

    async fn create_sandbox(
        &self,
        sandbox: Sandbox,
        assignment: Assignment,
        operation: Operation,
    ) -> StoreResult<()> {
        self.put_sandbox(&sandbox).await?;
        self.put_operation(&operation).await?;
        self.put_assignment(&assignment).await
    }

    async fn get_sandbox(&self, id: SandboxId) -> StoreResult<Sandbox> {
        let mut connection = self.pool.get().await.map_err(backend)?;
        let row = sandboxes::table
            .filter(sandboxes::id.eq(id.0))
            .select(sandboxes::record)
            .first::<Value>(&mut connection)
            .await
            .optional()
            .map_err(backend)?
            .ok_or_else(|| StoreError::NotFound(format!("sandbox {id}")))?;
        serde_json::from_value(row).map_err(Into::into)
    }

    async fn update_sandbox(&self, sandbox: Sandbox) -> StoreResult<()> {
        self.get_sandbox(sandbox.id).await?;
        self.put_sandbox(&sandbox).await
    }

    async fn list_sandboxes(&self, tenant: Option<&str>) -> StoreResult<Vec<Sandbox>> {
        let mut connection = self.pool.get().await.map_err(backend)?;
        let mut query = sandboxes::table.into_boxed();
        if let Some(tenant) = tenant {
            query = query.filter(sandboxes::tenant.eq(tenant));
        }
        let rows = query
            .order(sandboxes::updated_at.desc())
            .select(sandboxes::record)
            .load::<Value>(&mut connection)
            .await
            .map_err(backend)?;
        rows.into_iter()
            .map(|record| serde_json::from_value(record).map_err(Into::into))
            .collect()
    }

    async fn create_assignment(
        &self,
        assignment: Assignment,
        operation: Operation,
    ) -> StoreResult<()> {
        self.get_sandbox(assignment.sandbox_id).await?;
        self.put_operation(&operation).await?;
        self.put_assignment(&assignment).await
    }

    async fn lease_assignments(
        &self,
        node_id: NodeId,
        limit: usize,
        lease_seconds: i64,
    ) -> StoreResult<Vec<Assignment>> {
        let mut connection = self.pool.get().await.map_err(backend)?;
        let now = Utc::now();
        let rows = assignments::table
            .filter(assignments::node_id.eq(node_id.0))
            .filter(
                assignments::state.eq("pending").or(assignments::state
                    .eq("leased")
                    .and(assignments::lease_until.le(now))),
            )
            .order(assignments::created_at.asc())
            .limit(i64::try_from(limit.min(100)).map_err(backend)?)
            .select(assignments::record)
            .load::<Value>(&mut connection)
            .await
            .map_err(backend)?;
        drop(connection);
        let mut leased = Vec::with_capacity(rows.len());
        for record in rows {
            let mut assignment: Assignment = serde_json::from_value(record)?;
            assignment.state = AssignmentState::Leased;
            assignment.attempt = assignment.attempt.saturating_add(1);
            assignment.lease_until = Some(now + Duration::seconds(lease_seconds));
            self.put_assignment(&assignment).await?;
            let mut operation = self.get_operation(assignment.operation_id).await?;
            operation.state = OperationState::Running;
            operation.updated_at = now;
            self.put_operation(&operation).await?;
            leased.push(assignment);
        }
        Ok(leased)
    }

    async fn complete_assignment(&self, request: CompleteAssignmentRequest) -> StoreResult<()> {
        let mut connection = self.pool.get().await.map_err(backend)?;
        let row = assignments::table
            .filter(assignments::id.eq(request.assignment_id.0))
            .select(assignments::record)
            .first::<Value>(&mut connection)
            .await
            .optional()
            .map_err(backend)?
            .ok_or_else(|| StoreError::NotFound(format!("assignment {}", request.assignment_id)))?;
        drop(connection);
        let mut assignment: Assignment = serde_json::from_value(row)?;
        if assignment.operation_id != request.operation_id
            || assignment.sandbox_id != request.sandbox_id
        {
            return Err(StoreError::Conflict(
                "assignment completion identifiers do not match".into(),
            ));
        }
        assignment.state = if request.success {
            AssignmentState::Completed
        } else {
            AssignmentState::Failed
        };
        assignment.lease_until = None;
        self.put_assignment(&assignment).await?;
        let now = Utc::now();
        let mut operation = self.get_operation(request.operation_id).await?;
        operation.state = if request.success {
            OperationState::Succeeded
        } else {
            OperationState::Failed
        };
        operation.output = request.output;
        operation.error = request.error.clone();
        operation.updated_at = now;
        self.put_operation(&operation).await?;
        let mut sandbox = self.get_sandbox(request.sandbox_id).await?;
        sandbox.state = request.sandbox_state;
        sandbox.failure = request.error;
        sandbox.updated_at = now;
        self.put_sandbox(&sandbox).await
    }

    async fn get_operation(&self, id: OperationId) -> StoreResult<Operation> {
        let mut connection = self.pool.get().await.map_err(backend)?;
        let row = operations::table
            .filter(operations::id.eq(id.0))
            .select(operations::record)
            .first::<Value>(&mut connection)
            .await
            .optional()
            .map_err(backend)?
            .ok_or_else(|| StoreError::NotFound(format!("operation {id}")))?;
        serde_json::from_value(row).map_err(Into::into)
    }
}

fn state_name(state: AssignmentState) -> &'static str {
    match state {
        AssignmentState::Pending => "pending",
        AssignmentState::Leased => "leased",
        AssignmentState::Completed => "completed",
        AssignmentState::Failed => "failed",
    }
}

fn backend(error: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(error.to_string())
}
