use crate::config::{Condition, Config, FieldMapping, SyncTaskConfig};
use crate::error::Result;
use crate::source::postgres::connection::PostgresConnector;
use crate::source::postgres::full_sync::FullSyncHandler;
use std::collections::HashSet;
use tracing::{info, warn};

/// Validates the configuration and provides warnings/suggestions
pub struct ConfigValidator {
    config: Config,
}

impl ConfigValidator {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    /// Collect all field names referenced in a condition
    fn collect_fields_from_condition(condition: &Condition, fields: &mut HashSet<String>) {
        match condition {
            Condition::Equals { field, .. }
            | Condition::NotEquals { field, .. }
            | Condition::Contains { field, .. }
            | Condition::GreaterThan { field, .. }
            | Condition::LessThan { field, .. }
            | Condition::In { field, .. }
            | Condition::IsNull { field }
            | Condition::IsNotNull { field } => {
                fields.insert(field.clone());
            }
            Condition::And { conditions } | Condition::Or { conditions } => {
                for cond in conditions {
                    Self::collect_fields_from_condition(cond, fields);
                }
            }
        }
    }

    /// Validate the entire configuration
    pub fn validate(&self) -> Result<ValidationReport> {
        let mut report = ValidationReport::new();

        // Validate sync tasks
        self.validate_sync_tasks(&mut report)?;

        // Validate source configuration
        self.validate_source(&mut report)?;

        // Validate destination configuration
        self.validate_destination(&mut report)?;

        Ok(report)
    }

    /// Validate sync task configurations
    fn validate_sync_tasks(&self, report: &mut ValidationReport) -> Result<()> {
        let mut seen_tables = HashSet::new();
        let mut seen_indexes = HashSet::new();
        let mut cdc_only_tables = Vec::new();

        for task in &self.config.sync_tasks {
            // Check for duplicate tables
            if !seen_tables.insert(&task.table) {
                report.add_error(format!(
                    "Duplicate sync task for table '{}'. Only one sync task per table is allowed.",
                    task.table
                ));
            }

            // Check for duplicate indexes (warning only)
            if !seen_indexes.insert(&task.index) {
                report.add_warning(format!(
                    "Multiple tables syncing to the same index '{}'. This may cause conflicts.",
                    task.index
                ));
            }

            // Validate CDC-only tables
            if !task.full_sync_on_start.unwrap_or(false) {
                cdc_only_tables.push(&task.table);

                // Ensure auto_create_index is enabled for CDC-only tables
                if !self.config.meilisearch.auto_create_index {
                    report.add_warning(format!(
                        "Table '{}' is CDC-only but auto_create_index is disabled. \
                        The index won't be created until manually created in Meilisearch.",
                        task.table
                    ));
                }
            }

            // Validate filter configuration
            if let Some(filter) = &task.filter {
                self.validate_filter(task, filter, report)?;
            }

            // Validate batch configuration
            self.validate_batch_config(task, report)?;
        }

        // Report CDC-only tables
        if !cdc_only_tables.is_empty() {
            report.add_info(format!(
                "CDC-only tables (no initial full sync): {}",
                cdc_only_tables
                    .iter()
                    .map(|&t| t.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        Ok(())
    }

    /// Validate filter configuration
    fn validate_filter(
        &self,
        task: &SyncTaskConfig,
        filter: &crate::config::FilterConfig,
        report: &mut ValidationReport,
    ) -> Result<()> {
        // Check if filter conditions reference valid fields
        if let Some(conditions) = &filter.conditions {
            // Collect all fields referenced in conditions
            let mut referenced_fields = HashSet::new();
            for condition in conditions {
                Self::collect_fields_from_condition(condition, &mut referenced_fields);
            }

            // Store fields for validation (will be validated in validate_fields_against_schema)
            report.add_info(format!(
                "Filter for table '{}' references fields: {:?}",
                task.table, referenced_fields
            ));
        }

        // Validate event types
        if let Some(event_types) = &filter.event_types
            && event_types.is_empty()
        {
            report.add_warning(format!(
                "Empty event_types filter for table '{}' will filter out all events",
                task.table
            ));
        }

        Ok(())
    }

    /// Validate batch configuration
    fn validate_batch_config(
        &self,
        task: &SyncTaskConfig,
        report: &mut ValidationReport,
    ) -> Result<()> {
        if task.options.batch_size == 0 {
            report.add_error(format!(
                "Invalid batch_size (0) for table '{}'. Must be greater than 0.",
                task.table
            ));
        }

        if task.options.batch_size > 10000 {
            report.add_warning(format!(
                "Large batch_size ({}) for table '{}' may cause memory issues",
                task.options.batch_size, task.table
            ));
        }

        if task.options.batch_timeout_ms < 100 {
            report.add_warning(format!(
                "Very low batch_timeout_ms ({}) for table '{}' may cause excessive API calls",
                task.options.batch_timeout_ms, task.table
            ));
        }

        Ok(())
    }

    /// Validate source configuration
    fn validate_source(&self, report: &mut ValidationReport) -> Result<()> {
        // Validate single source if present
        if let Some(source) = &self.config.source {
            match source {
                crate::config::SourceConfig::PostgreSQL(pg_config) => {
                    // Validate slot name
                    if pg_config.slot_name.is_empty() {
                        report.add_error("PostgreSQL slot_name cannot be empty".to_string());
                    }

                    // Validate publication name
                    if pg_config.publication.is_empty() {
                        report.add_error("PostgreSQL publication name cannot be empty".to_string());
                    }

                    // Validate pool configuration
                    if pg_config.pool.min_idle > pg_config.pool.max_size {
                        report.add_error(format!(
                            "PostgreSQL pool min_idle ({}) cannot be greater than max_size ({})",
                            pg_config.pool.min_idle, pg_config.pool.max_size
                        ));
                    }
                }
                _ => {
                    report.add_warning(
                        "Non-PostgreSQL sources are not yet fully supported".to_string(),
                    );
                }
            }
        }

        // Validate multiple sources
        for named_source in &self.config.sources {
            match &named_source.config {
                crate::config::SourceConfig::PostgreSQL(pg_config) => {
                    // Validate slot name
                    if pg_config.slot_name.is_empty() {
                        report.add_error(format!(
                            "PostgreSQL source '{}' slot_name cannot be empty",
                            named_source.name
                        ));
                    }

                    // Validate publication name
                    if pg_config.publication.is_empty() {
                        report.add_error(format!(
                            "PostgreSQL source '{}' publication name cannot be empty",
                            named_source.name
                        ));
                    }

                    // Validate pool configuration
                    if pg_config.pool.min_idle > pg_config.pool.max_size {
                        report.add_error(format!(
                            "PostgreSQL source '{}' pool min_idle ({}) cannot be greater than max_size ({})",
                            named_source.name, pg_config.pool.min_idle, pg_config.pool.max_size
                        ));
                    }
                }
                _ => {
                    report.add_warning(format!(
                        "Non-PostgreSQL source '{}' is not yet fully supported",
                        named_source.name
                    ));
                }
            }
        }

        // Ensure at least one source is configured
        if self.config.source.is_none() && self.config.sources.is_empty() {
            report.add_error(
                "At least one data source must be configured (use 'source' or 'sources')"
                    .to_string(),
            );
        }

        Ok(())
    }

    /// Validate destination configuration
    fn validate_destination(&self, report: &mut ValidationReport) -> Result<()> {
        if self.config.meilisearch.url.is_empty() {
            report.add_error("Meilisearch URL cannot be empty".to_string());
        }

        if self
            .config
            .meilisearch
            .api_key
            .as_ref()
            .map(|k| k.is_empty())
            .unwrap_or(false)
        {
            report.add_warning(
                "Meilisearch API key is empty - ensure Meilisearch allows anonymous access"
                    .to_string(),
            );
        }

        if self.config.meilisearch.timeout == 0 {
            report.add_error("Meilisearch timeout must be greater than 0".to_string());
        }

        Ok(())
    }

    /// Validate fields against actual PostgreSQL table schema
    pub async fn validate_fields_against_schema(
        &self,
        report: &mut ValidationReport,
    ) -> Result<()> {
        // Only validate if we have a PostgreSQL source
        let pg_config = match &self.config.source {
            Some(crate::config::SourceConfig::PostgreSQL(config)) => config,
            _ => return Ok(()), // Skip validation for non-PostgreSQL sources
        };

        // Create a connector to query schema
        let mut connector = PostgresConnector::new((**pg_config).clone());
        if let Err(e) = connector.connect().await {
            report.add_warning(format!("Cannot validate fields against schema: {e}"));
            return Ok(());
        }

        let full_sync_handler = FullSyncHandler::new(connector.clone());

        // Validate each sync task
        for task in &self.config.sync_tasks {
            // Get table columns
            match full_sync_handler.get_table_columns(&task.table).await {
                Ok(columns) => {
                    let column_set: HashSet<String> = columns.into_iter().collect();

                    // Validate filter fields
                    if let Some(filter) = &task.filter {
                        if let Some(conditions) = &filter.conditions {
                            let mut referenced_fields = HashSet::new();
                            for condition in conditions {
                                Self::collect_fields_from_condition(
                                    condition,
                                    &mut referenced_fields,
                                );
                            }

                            // Check if referenced fields exist in mapping
                            if let Some(mapping) = &task.mapping {
                                if let Some(tables) = &mapping.tables.get(&task.table) {
                                    if let Some(fields) = &tables.fields {
                                        for field in referenced_fields {
                                            if !fields.contains_key(&field) {
                                                report.add_warning(format!(
                                                    "Table '{}': Filter condition references field '{}' which is not in the mapping",
                                                    task.table, field
                                                ));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Validate mapping fields
                    if let Some(mapping) = &task.mapping {
                        if let Some(tables) = &mapping.tables.get(&task.table) {
                            if let Some(fields) = &tables.fields {
                                for (from_field, mapping_config) in fields {
                                    // Validate source field
                                    if !column_set.contains(from_field) {
                                        report.add_error(format!(
                                            "Table '{}': Mapping references non-existent source field '{}'",
                                            task.table, from_field
                                        ));
                                    }

                                    // Validate complex mapping sources
                                    if let FieldMapping::Complex {
                                        sources: Some(sources),
                                        ..
                                    } = mapping_config
                                    {
                                        for source in sources {
                                            if !column_set.contains(source) {
                                                report.add_error(format!(
                                                    "Table '{}': Complex mapping references non-existent source field '{}'",
                                                    task.table, source
                                                ));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Validate transform fields
                    if let Some(transform) = &task.transform {
                        if let Some(table_transforms) = transform.fields.get(&task.table) {
                            for field_transform in table_transforms.values() {
                                if let crate::config::pipeline::FieldTransform::Extract {
                                    from,
                                    path,
                                    ..
                                } = field_transform
                                {
                                    if !column_set.contains(from) {
                                        report.add_error(format!(
                                            "Table '{}': Transform extracts from non-existent field '{}'",
                                            task.table, from
                                        ));
                                    }
                                    if path.is_empty() {
                                        report.add_error(format!(
                                            "Table '{}': Transform has empty extraction path",
                                            task.table
                                        ));
                                    }
                                }
                            }
                        }
                    }

                    // Validate soft delete field
                    if let Some(soft_delete) = &task.soft_delete
                        && !column_set.contains(&soft_delete.field)
                    {
                        report.add_error(format!(
                            "Table '{}': Soft delete references non-existent field '{}'",
                            task.table, soft_delete.field
                        ));
                    }

                    report.add_info(format!(
                        "Table '{}' has {} columns: {:?}",
                        task.table,
                        column_set.len(),
                        column_set
                    ));
                }
                Err(e) => {
                    report.add_warning(format!(
                        "Cannot validate fields for table '{}': {}",
                        task.table, e
                    ));
                }
            }
        }

        // Disconnect
        connector.disconnect().await;

        Ok(())
    }
}

/// Report of validation results
#[derive(Debug)]
pub struct ValidationReport {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub info: Vec<String>,
}

impl ValidationReport {
    fn new() -> Self {
        Self {
            errors: Vec::new(),
            warnings: Vec::new(),
            info: Vec::new(),
        }
    }

    fn add_error(&mut self, error: String) {
        self.errors.push(error);
    }

    fn add_warning(&mut self, warning: String) {
        self.warnings.push(warning);
    }

    fn add_info(&mut self, info: String) {
        self.info.push(info);
    }

    /// Check if configuration is valid (no errors)
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    /// Print the validation report
    pub fn print(&self) {
        if !self.errors.is_empty() {
            warn!("Configuration validation errors:");
            for error in &self.errors {
                warn!("  ❌ {}", error);
            }
        }

        if !self.warnings.is_empty() {
            warn!("Configuration warnings:");
            for warning in &self.warnings {
                warn!("  ⚠️  {}", warning);
            }
        }

        if !self.info.is_empty() {
            info!("Configuration info:");
            for info in &self.info {
                info!("  ℹ️  {}", info);
            }
        }

        if self.is_valid() {
            info!("✅ Configuration validation passed");
        }
    }
}
