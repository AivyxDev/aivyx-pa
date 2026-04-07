//! Finance Tracking — bill detection, expense categorization, budget awareness.
//!
//! Transactions (bills, expenses, income) are stored encrypted in the local
//! store under `finance-tx:{uuid}`. Budget rules live under `finance-budget:{uuid}`.
//! A scan cursor (`finance-scan-cursor`) tracks the last processed email
//! sequence number to avoid re-scanning.
//!
//! The LLM handles categorization — no rules engine needed. The agent detects
//! financial emails, extracts structured data, and asks for confirmation.

use aivyx_core::Result;
use aivyx_crypto::{EncryptedStore, MasterKey};
use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::Action;

// ── Data structures ─────────────────────────────────────────────

/// A financial transaction — bill, expense, or income.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub id: String,
    pub kind: TransactionKind,
    /// Amount in cents (avoids floating-point rounding).
    pub amount_cents: i64,
    /// ISO 4217 currency code (default: "USD").
    pub currency: String,
    pub description: String,
    /// Category (e.g., "dining", "utilities", "groceries").
    pub category: Option<String>,
    /// Vendor name (e.g., "PG&E", "Starbucks").
    pub vendor: Option<String>,
    /// Due date — primarily for bills.
    pub due_date: Option<DateTime<Utc>>,
    /// When the transaction occurred or was detected.
    pub transaction_date: DateTime<Utc>,
    /// How this transaction was created.
    pub source: TransactionSource,
    /// Link back to the originating email, if any.
    pub email_message_id: Option<String>,
    /// Path in the document vault if a receipt was filed.
    pub receipt_path: Option<String>,
    /// For bills: has it been paid?
    pub paid: bool,
    /// User-confirmed? Auto-detected transactions start unconfirmed.
    pub confirmed: bool,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TransactionKind {
    Bill,
    Expense,
    Income,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransactionSource {
    EmailDetection,
    Manual,
    BankNotification,
}

/// Monthly budget limit for a category.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetRule {
    pub id: String,
    pub category: String,
    pub monthly_limit_cents: i64,
    pub created_at: DateTime<Utc>,
}

// ── Store helpers ───────────────────────────────────────────────

const TX_PREFIX: &str = "finance-tx:";
const BUDGET_PREFIX: &str = "finance-budget:";
const SCAN_CURSOR_KEY: &str = "finance-scan-cursor";

/// Save a transaction to the encrypted store.
pub fn save_transaction(
    store: &EncryptedStore,
    key: &MasterKey,
    tx: &Transaction,
) -> Result<()> {
    let json = serde_json::to_vec(tx)
        .map_err(aivyx_core::AivyxError::Serialization)?;
    store.put(&format!("{TX_PREFIX}{}", tx.id), &json, key)
}

/// Load all transactions from the encrypted store.
pub fn load_all_transactions(
    store: &EncryptedStore,
    key: &MasterKey,
) -> Result<Vec<Transaction>> {
    let keys = store.list_keys()?;
    let mut txs = Vec::new();
    for k in &keys {
        if !k.starts_with(TX_PREFIX) {
            continue;
        }
        if let Some(bytes) = store.get(k, key)? {
            match serde_json::from_slice::<Transaction>(&bytes) {
                Ok(tx) => txs.push(tx),
                Err(e) => tracing::warn!("Corrupt finance transaction '{k}': {e}"),
            }
        }
    }
    txs.sort_by(|a, b| b.transaction_date.cmp(&a.transaction_date));
    Ok(txs)
}

/// Load transactions for a specific month (YYYY-MM).
pub fn load_transactions_for_month(
    store: &EncryptedStore,
    key: &MasterKey,
    year: i32,
    month: u32,
) -> Result<Vec<Transaction>> {
    let all = load_all_transactions(store, key)?;
    Ok(all
        .into_iter()
        .filter(|tx| {
            tx.transaction_date.year() == year && tx.transaction_date.month() == month
        })
        .collect())
}

/// Delete a transaction by ID.
pub fn delete_transaction(
    store: &EncryptedStore,
    id: &str,
) -> Result<()> {
    store.delete(&format!("{TX_PREFIX}{id}"))
}

/// Save a budget rule.
pub fn save_budget_rule(
    store: &EncryptedStore,
    key: &MasterKey,
    rule: &BudgetRule,
) -> Result<()> {
    let json = serde_json::to_vec(rule)
        .map_err(aivyx_core::AivyxError::Serialization)?;
    store.put(&format!("{BUDGET_PREFIX}{}", rule.id), &json, key)
}

/// Load all budget rules.
pub fn load_budget_rules(
    store: &EncryptedStore,
    key: &MasterKey,
) -> Result<Vec<BudgetRule>> {
    let keys = store.list_keys()?;
    let mut rules = Vec::new();
    for k in &keys {
        if !k.starts_with(BUDGET_PREFIX) {
            continue;
        }
        if let Some(bytes) = store.get(k, key)? {
            match serde_json::from_slice::<BudgetRule>(&bytes) {
                Ok(rule) => rules.push(rule),
                Err(e) => tracing::warn!("Corrupt budget rule '{k}': {e}"),
            }
        }
    }
    Ok(rules)
}

/// Load the email scan cursor (last processed sequence number).
pub fn load_scan_cursor(store: &EncryptedStore, key: &MasterKey) -> Result<u32> {
    match store.get(SCAN_CURSOR_KEY, key)? {
        Some(bytes) => {
            let s = String::from_utf8(bytes)
                .map_err(|e| aivyx_core::AivyxError::Other(format!("Invalid cursor: {e}")))?;
            s.trim()
                .parse::<u32>()
                .map_err(|e| aivyx_core::AivyxError::Other(format!("Invalid cursor: {e}")))
        }
        None => Ok(0),
    }
}

/// Save the email scan cursor.
pub fn save_scan_cursor(store: &EncryptedStore, key: &MasterKey, seq: u32) -> Result<()> {
    store.put(SCAN_CURSOR_KEY, seq.to_string().as_bytes(), key)
}

// ── Dollar/cents conversion ─────────────────────────────────────

/// Convert a dollar amount (f64) to cents (i64).
///
/// Uses string-based rounding to avoid floating-point precision issues.
/// For example, `19.995 * 100.0` produces `1999.4999...` which rounds to
/// 1999 instead of the expected 2000. Formatting to 2 decimal places first
/// ensures banker-friendly rounding.
fn dollars_to_cents(dollars: f64) -> i64 {
    let s = format!("{dollars:.2}");
    if let Some((whole, frac)) = s.split_once('.') {
        let sign = if dollars < 0.0 { -1i64 } else { 1 };
        let w: i64 = whole.trim_start_matches('-').parse().unwrap_or(0);
        let f: i64 = frac.get(..2).unwrap_or(frac).parse().unwrap_or(0);
        sign * (w * 100 + f)
    } else {
        (dollars * 100.0).round() as i64
    }
}

/// Format cents as a dollar string (e.g., 14230 → "$142.30").
pub fn format_dollars(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let abs = cents.unsigned_abs();
    format!("{sign}${}.{:02}", abs / 100, abs % 100)
}

/// Parse a month string "YYYY-MM" into (year, month). Falls back to current month.
fn parse_month(s: Option<&str>) -> (i32, u32) {
    if let Some(s) = s {
        let parts: Vec<&str> = s.split('-').collect();
        if parts.len() == 2
            && let (Ok(y), Ok(m)) = (parts[0].parse::<i32>(), parts[1].parse::<u32>())
                && (1..=12).contains(&m) {
                    return (y, m);
                }
    }
    let now = Utc::now();
    (now.year(), now.month())
}

// ── Action tools ────────────────────────────────────────────────

/// Tool: add a transaction manually.
pub struct AddTransaction {
    pub store: Arc<EncryptedStore>,
    pub key: MasterKey,
}

#[async_trait::async_trait]
impl Action for AddTransaction {
    fn name(&self) -> &str { "add_transaction" }

    fn description(&self) -> &str {
        "Record a financial transaction (expense, bill, or income). \
         Use this when the user mentions spending money, receiving a bill, \
         or getting paid."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": ["bill", "expense", "income"],
                    "description": "Type of transaction"
                },
                "amount": {
                    "type": "number",
                    "description": "Amount in dollars (e.g., 142.30)"
                },
                "description": {
                    "type": "string",
                    "description": "What this transaction is for"
                },
                "category": {
                    "type": "string",
                    "description": "Category: dining, groceries, utilities, housing, transport, entertainment, health, shopping, subscriptions, education, travel, other"
                },
                "vendor": {
                    "type": "string",
                    "description": "Vendor or company name"
                },
                "due_date": {
                    "type": "string",
                    "description": "ISO date for bills (e.g., 2026-04-07)"
                },
                "currency": {
                    "type": "string",
                    "description": "Currency code (default: USD)"
                }
            },
            "required": ["kind", "amount", "description"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let kind_str = input["kind"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'kind' is required".into()))?;
        let amount = input["amount"]
            .as_f64()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'amount' is required".into()))?;
        let description = input["description"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'description' is required".into()))?;

        let kind = match kind_str {
            "bill" => TransactionKind::Bill,
            "expense" => TransactionKind::Expense,
            "income" => TransactionKind::Income,
            other => return Err(aivyx_core::AivyxError::Validation(
                format!("Invalid kind '{other}', expected: bill, expense, income"),
            )),
        };

        let due_date = input["due_date"]
            .as_str()
            .and_then(|s| {
                chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                    .ok()
                    .map(|d| d.and_hms_opt(0, 0, 0).unwrap())
                    .map(|dt| DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc))
            });

        let tx = Transaction {
            id: uuid::Uuid::new_v4().to_string(),
            kind,
            amount_cents: dollars_to_cents(amount),
            currency: input["currency"].as_str().unwrap_or("USD").to_string(),
            description: description.to_string(),
            category: input["category"].as_str().map(|s| s.to_lowercase()),
            vendor: input["vendor"].as_str().map(String::from),
            due_date,
            transaction_date: Utc::now(),
            source: TransactionSource::Manual,
            email_message_id: None,
            receipt_path: None,
            paid: false,
            confirmed: true, // manual entries are pre-confirmed
            tags: vec![],
            created_at: Utc::now(),
        };

        save_transaction(&self.store, &self.key, &tx)?;

        Ok(serde_json::json!({
            "status": "recorded",
            "id": tx.id,
            "amount": format_dollars(tx.amount_cents),
            "kind": input["kind"],
            "description": tx.description,
            "category": tx.category,
        }))
    }
}

/// Tool: list transactions with optional filtering.
pub struct ListTransactions {
    pub store: Arc<EncryptedStore>,
    pub key: MasterKey,
}

#[async_trait::async_trait]
impl Action for ListTransactions {
    fn name(&self) -> &str { "list_transactions" }

    fn description(&self) -> &str {
        "List financial transactions. Filter by month, category, or type. \
         Defaults to the current month."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "month": {
                    "type": "string",
                    "description": "YYYY-MM (default: current month)"
                },
                "category": {
                    "type": "string",
                    "description": "Filter by category"
                },
                "kind": {
                    "type": "string",
                    "enum": ["bill", "expense", "income"],
                    "description": "Filter by transaction type"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max results (default: 50)"
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let (year, month) = parse_month(input["month"].as_str());
        let limit = input["limit"].as_u64().unwrap_or(50) as usize;
        let filter_category = input["category"].as_str().map(|s| s.to_lowercase());
        let filter_kind = input["kind"].as_str();

        let mut txs = load_transactions_for_month(&self.store, &self.key, year, month)?;

        if let Some(cat) = &filter_category {
            txs.retain(|tx| tx.category.as_deref() == Some(cat));
        }
        if let Some(kind) = filter_kind {
            let k = match kind {
                "bill" => TransactionKind::Bill,
                "expense" => TransactionKind::Expense,
                "income" => TransactionKind::Income,
                _ => return Err(aivyx_core::AivyxError::Validation(
                    format!("Invalid kind '{kind}'"),
                )),
            };
            txs.retain(|tx| tx.kind == k);
        }

        txs.truncate(limit);

        let entries: Vec<serde_json::Value> = txs
            .iter()
            .map(|tx| {
                serde_json::json!({
                    "id": tx.id,
                    "kind": format!("{:?}", tx.kind).to_lowercase(),
                    "amount": format_dollars(tx.amount_cents),
                    "description": tx.description,
                    "category": tx.category,
                    "vendor": tx.vendor,
                    "date": tx.transaction_date.format("%Y-%m-%d").to_string(),
                    "due_date": tx.due_date.map(|d| d.format("%Y-%m-%d").to_string()),
                    "paid": tx.paid,
                    "confirmed": tx.confirmed,
                })
            })
            .collect();

        let total_cents: i64 = txs.iter().map(|tx| tx.amount_cents).sum();

        Ok(serde_json::json!({
            "transactions": entries,
            "count": entries.len(),
            "total": format_dollars(total_cents),
            "month": format!("{year}-{month:02}"),
        }))
    }
}

/// Tool: budget summary — spending totals by category for a month.
pub struct BudgetSummary {
    pub store: Arc<EncryptedStore>,
    pub key: MasterKey,
}

#[async_trait::async_trait]
impl Action for BudgetSummary {
    fn name(&self) -> &str { "budget_summary" }

    fn description(&self) -> &str {
        "Show spending totals by category for a month, compared against \
         budget limits if set. Use this when the user asks about their spending."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "month": {
                    "type": "string",
                    "description": "YYYY-MM (default: current month)"
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let (year, month) = parse_month(input["month"].as_str());
        let txs = load_transactions_for_month(&self.store, &self.key, year, month)?;
        let rules = load_budget_rules(&self.store, &self.key)?;

        // Group spending by category (expenses + bills only, not income).
        let mut by_category: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();
        let mut total_expenses = 0i64;
        let mut total_income = 0i64;
        let mut unpaid_bills = Vec::new();

        for tx in &txs {
            match tx.kind {
                TransactionKind::Income => {
                    total_income += tx.amount_cents;
                }
                TransactionKind::Bill | TransactionKind::Expense => {
                    total_expenses += tx.amount_cents;
                    let cat = tx.category.clone().unwrap_or_else(|| "other".into());
                    *by_category.entry(cat).or_default() += tx.amount_cents;
                }
            }
            if tx.kind == TransactionKind::Bill && !tx.paid {
                unpaid_bills.push(serde_json::json!({
                    "description": tx.description,
                    "amount": format_dollars(tx.amount_cents),
                    "due_date": tx.due_date.map(|d| d.format("%Y-%m-%d").to_string()),
                }));
            }
        }

        // Build per-category breakdown with budget comparison.
        let rule_map: std::collections::HashMap<&str, i64> = rules
            .iter()
            .map(|r| (r.category.as_str(), r.monthly_limit_cents))
            .collect();

        let mut categories: Vec<serde_json::Value> = by_category
            .iter()
            .map(|(cat, &spent)| {
                let mut entry = serde_json::json!({
                    "category": cat,
                    "spent": format_dollars(spent),
                    "spent_cents": spent,
                });
                if let Some(&limit) = rule_map.get(cat.as_str()) {
                    entry["budget"] = serde_json::json!(format_dollars(limit));
                    entry["remaining"] = serde_json::json!(format_dollars(limit - spent));
                    entry["over_budget"] = serde_json::json!(spent > limit);
                }
                entry
            })
            .collect();
        categories.sort_by(|a, b| {
            b["spent_cents"].as_i64().cmp(&a["spent_cents"].as_i64())
        });

        Ok(serde_json::json!({
            "month": format!("{year}-{month:02}"),
            "total_expenses": format_dollars(total_expenses),
            "total_income": format_dollars(total_income),
            "net": format_dollars(total_income - total_expenses),
            "categories": categories,
            "unpaid_bills": unpaid_bills,
            "transaction_count": txs.len(),
        }))
    }
}

/// Tool: set a monthly budget limit for a category.
pub struct SetBudget {
    pub store: Arc<EncryptedStore>,
    pub key: MasterKey,
}

#[async_trait::async_trait]
impl Action for SetBudget {
    fn name(&self) -> &str { "set_budget" }

    fn description(&self) -> &str {
        "Set a monthly spending limit for a category. The budget_summary tool \
         will show spending vs. this limit."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "category": {
                    "type": "string",
                    "description": "Category name (e.g., 'dining', 'groceries')"
                },
                "monthly_limit": {
                    "type": "number",
                    "description": "Monthly limit in dollars (e.g., 500.00)"
                }
            },
            "required": ["category", "monthly_limit"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let category = input["category"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'category' is required".into()))?
            .to_lowercase();
        let limit = input["monthly_limit"]
            .as_f64()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'monthly_limit' is required".into()))?;

        // Check if a rule already exists for this category and update it.
        let existing = load_budget_rules(&self.store, &self.key)?;
        let rule = if let Some(existing_rule) = existing.iter().find(|r| r.category == category) {
            BudgetRule {
                id: existing_rule.id.clone(),
                category: category.clone(),
                monthly_limit_cents: dollars_to_cents(limit),
                created_at: existing_rule.created_at,
            }
        } else {
            BudgetRule {
                id: uuid::Uuid::new_v4().to_string(),
                category: category.clone(),
                monthly_limit_cents: dollars_to_cents(limit),
                created_at: Utc::now(),
            }
        };

        save_budget_rule(&self.store, &self.key, &rule)?;

        Ok(serde_json::json!({
            "status": "budget_set",
            "category": category,
            "monthly_limit": format_dollars(rule.monthly_limit_cents),
        }))
    }
}

/// Tool: mark a bill as paid.
pub struct MarkBillPaid {
    pub store: Arc<EncryptedStore>,
    pub key: MasterKey,
}

#[async_trait::async_trait]
impl Action for MarkBillPaid {
    fn name(&self) -> &str { "mark_bill_paid" }

    fn description(&self) -> &str {
        "Mark a bill transaction as paid. Use after the user confirms payment."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "transaction_id": {
                    "type": "string",
                    "description": "The transaction ID to mark as paid"
                }
            },
            "required": ["transaction_id"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let id = input["transaction_id"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'transaction_id' is required".into()))?;

        let store_key = format!("{TX_PREFIX}{id}");
        let bytes = self.store.get(&store_key, &self.key)?
            .ok_or_else(|| aivyx_core::AivyxError::Other(
                format!("Transaction '{id}' not found"),
            ))?;

        let mut tx: Transaction = serde_json::from_slice(&bytes)
            .map_err(|e| aivyx_core::AivyxError::Other(format!("Corrupt transaction: {e}")))?;

        if tx.kind != TransactionKind::Bill {
            return Err(aivyx_core::AivyxError::Validation(
                format!("Transaction '{id}' is not a bill (it's a {:?})", tx.kind),
            ));
        }

        tx.paid = true;
        save_transaction(&self.store, &self.key, &tx)?;

        Ok(serde_json::json!({
            "status": "marked_paid",
            "id": tx.id,
            "description": tx.description,
            "amount": format_dollars(tx.amount_cents),
        }))
    }
}

/// Tool: file a receipt email to the document vault.
pub struct FileReceipt {
    pub store: Arc<EncryptedStore>,
    pub key: MasterKey,
    pub email_config: crate::email::EmailConfig,
    pub vault_path: std::path::PathBuf,
    pub receipt_folder: String,
}

#[async_trait::async_trait]
impl Action for FileReceipt {
    fn name(&self) -> &str { "file_receipt" }

    fn description(&self) -> &str {
        "Save a receipt email to the document vault for future reference. \
         Fetches the email body and saves it as a text file in the receipts folder."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "email_seq": {
                    "type": "integer",
                    "description": "Email sequence number (from read_email results)"
                },
                "filename": {
                    "type": "string",
                    "description": "Filename for the receipt (e.g., 'electric-bill-apr-2026'). Extension .txt is added automatically."
                },
                "transaction_id": {
                    "type": "string",
                    "description": "Optional: link this receipt to an existing transaction"
                }
            },
            "required": ["email_seq", "filename"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let seq = input["email_seq"]
            .as_u64()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'email_seq' is required".into()))? as u32;
        let filename = input["filename"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'filename' is required".into()))?;

        // Fetch the full email.
        let email = crate::email::fetch_single(&self.email_config, seq).await?;

        // Build the receipt directory: vault/receipts/YYYY/MM/
        let now = Utc::now();
        let receipt_dir = self.vault_path
            .join(&self.receipt_folder)
            .join(now.format("%Y").to_string())
            .join(now.format("%m").to_string());

        std::fs::create_dir_all(&receipt_dir)
            .map_err(|e| aivyx_core::AivyxError::Other(format!("Failed to create receipt dir: {e}")))?;

        // Sanitize filename.
        let safe_name: String = filename
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
            .collect();
        let file_path = receipt_dir.join(format!("{safe_name}.txt"));

        // Write the email as a receipt file.
        let content = format!(
            "From: {}\nSubject: {}\nDate: {}\nMessage-ID: {}\n\n---\n\n{}",
            email.from,
            email.subject,
            email.date,
            email.message_id.as_deref().unwrap_or(""),
            email.body,
        );
        std::fs::write(&file_path, &content)
            .map_err(|e| aivyx_core::AivyxError::Other(format!("Failed to write receipt: {e}")))?;

        let relative_path = file_path
            .strip_prefix(&self.vault_path)
            .unwrap_or(&file_path)
            .to_string_lossy()
            .to_string();

        // Link to transaction if specified.
        if let Some(tx_id) = input["transaction_id"].as_str() {
            let store_key = format!("{TX_PREFIX}{tx_id}");
            if let Ok(Some(bytes)) = self.store.get(&store_key, &self.key)
                && let Ok(mut tx) = serde_json::from_slice::<Transaction>(&bytes) {
                    tx.receipt_path = Some(relative_path.clone());
                    let _ = save_transaction(&self.store, &self.key, &tx);
                }
        }

        Ok(serde_json::json!({
            "status": "filed",
            "path": relative_path,
            "subject": email.subject,
            "from": email.from,
        }))
    }
}

// ── Financial email detection ───────────────────────────────────

/// Keywords that suggest an email is financial.
const FINANCIAL_KEYWORDS: &[&str] = &[
    "bill", "invoice", "payment", "receipt", "transaction", "statement",
    "due", "balance", "charge", "refund", "subscription", "renewal",
    "amount due", "pay now", "autopay", "direct debit",
];

/// Check if an email subject or sender suggests financial content.
pub fn is_likely_financial(subject: &str, from: &str) -> bool {
    let subject_lower = subject.to_lowercase();
    let from_lower = from.to_lowercase();

    // Check subject for financial keywords.
    if FINANCIAL_KEYWORDS.iter().any(|kw| subject_lower.contains(kw)) {
        return true;
    }

    // Check common financial sender patterns.
    let financial_senders = [
        "noreply@", "billing@", "payments@", "receipts@", "invoices@",
        "statements@", "accounts@", "paypal", "venmo", "stripe",
        "square", "bank", "chase", "wells fargo", "amex",
    ];
    financial_senders.iter().any(|pat| from_lower.contains(pat))
}

/// Summary of bills upcoming within a given number of days.
/// Used by briefing and heartbeat for proactive alerts.
pub fn upcoming_bills(
    store: &EncryptedStore,
    key: &MasterKey,
    within_days: i64,
) -> Result<Vec<Transaction>> {
    let all = load_all_transactions(store, key)?;
    let cutoff = Utc::now() + chrono::Duration::days(within_days);
    let now = Utc::now();

    Ok(all
        .into_iter()
        .filter(|tx| {
            tx.kind == TransactionKind::Bill
                && !tx.paid
                && tx.due_date.is_some_and(|d| d <= cutoff && d >= now)
        })
        .collect())
}

/// Categories that have exceeded their budget for the current month.
pub fn over_budget_categories(
    store: &EncryptedStore,
    key: &MasterKey,
) -> Result<Vec<(String, i64, i64)>> {
    let now = Utc::now();
    let txs = load_transactions_for_month(store, key, now.year(), now.month())?;
    let rules = load_budget_rules(store, key)?;

    let mut by_category: std::collections::HashMap<String, i64> =
        std::collections::HashMap::new();
    for tx in &txs {
        if tx.kind != TransactionKind::Income {
            let cat = tx.category.clone().unwrap_or_else(|| "other".into());
            *by_category.entry(cat).or_default() += tx.amount_cents;
        }
    }

    let mut over = Vec::new();
    for rule in &rules {
        let spent = by_category.get(&rule.category).copied().unwrap_or(0);
        if spent > rule.monthly_limit_cents {
            over.push((rule.category.clone(), spent, rule.monthly_limit_cents));
        }
    }
    Ok(over)
}

// ── CUD tools ─────────────────────────────────────────────────────

/// Tool: delete a transaction by ID.
pub struct DeleteTransactionAction {
    pub store: Arc<EncryptedStore>,
    pub key: MasterKey,
}

#[async_trait::async_trait]
impl Action for DeleteTransactionAction {
    fn name(&self) -> &str { "delete_transaction" }

    fn description(&self) -> &str {
        "Permanently delete a financial transaction by ID."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["id"],
            "properties": {
                "id": { "type": "string", "description": "Transaction ID to delete" }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let id = input["id"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'id' is required".into()))?;

        // Verify it exists
        let key_str = format!("{TX_PREFIX}{id}");
        if self.store.get(&key_str, &self.key)?.is_none() {
            return Err(aivyx_core::AivyxError::Validation(format!("Transaction '{id}' not found")));
        }

        delete_transaction(&self.store, id)?;

        Ok(serde_json::json!({
            "status": "deleted",
            "id": id,
        }))
    }
}

/// Tool: update fields on an existing transaction.
pub struct UpdateTransaction {
    pub store: Arc<EncryptedStore>,
    pub key: MasterKey,
}

#[async_trait::async_trait]
impl Action for UpdateTransaction {
    fn name(&self) -> &str { "update_transaction" }

    fn description(&self) -> &str {
        "Update fields on an existing transaction. Only the fields you provide will be changed."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["id"],
            "properties": {
                "id": { "type": "string", "description": "Transaction ID to update" },
                "description": { "type": "string" },
                "category": { "type": "string" },
                "amount_cents": { "type": "integer" },
                "vendor": { "type": "string" },
                "paid": { "type": "boolean" },
                "confirmed": { "type": "boolean" },
                "tags": { "type": "array", "items": { "type": "string" } }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let id = input["id"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'id' is required".into()))?;

        let key_str = format!("{TX_PREFIX}{id}");
        let bytes = self.store.get(&key_str, &self.key)?
            .ok_or_else(|| aivyx_core::AivyxError::Validation(format!("Transaction '{id}' not found")))?;
        let mut tx: Transaction = serde_json::from_slice(&bytes)
            .map_err(|e| aivyx_core::AivyxError::Other(format!("Corrupt transaction: {e}")))?;

        if let Some(v) = input["description"].as_str() { tx.description = v.into(); }
        if !input["category"].is_null() { tx.category = input["category"].as_str().map(String::from); }
        if let Some(v) = input["amount_cents"].as_i64() { tx.amount_cents = v; }
        if !input["vendor"].is_null() { tx.vendor = input["vendor"].as_str().map(String::from); }
        if let Some(v) = input["paid"].as_bool() { tx.paid = v; }
        if let Some(v) = input["confirmed"].as_bool() { tx.confirmed = v; }
        if let Some(a) = input["tags"].as_array() {
            tx.tags = a.iter().filter_map(|v| v.as_str().map(String::from)).collect();
        }

        save_transaction(&self.store, &self.key, &tx)?;

        Ok(serde_json::json!({
            "status": "updated",
            "id": id,
        }))
    }
}

/// Tool: delete a budget rule by ID.
pub struct DeleteBudget {
    pub store: Arc<EncryptedStore>,
}

#[async_trait::async_trait]
impl Action for DeleteBudget {
    fn name(&self) -> &str { "delete_budget" }

    fn description(&self) -> &str {
        "Delete a budget rule by ID. Use budget_summary to see current budget rules."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["id"],
            "properties": {
                "id": { "type": "string", "description": "Budget rule ID to delete" }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let id = input["id"]
            .as_str()
            .ok_or_else(|| aivyx_core::AivyxError::Validation("'id' is required".into()))?;

        self.store.delete(&format!("{BUDGET_PREFIX}{id}"))?;

        Ok(serde_json::json!({
            "status": "deleted",
            "id": id,
        }))
    }
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Dollar/cents conversion ─────────────────────────────────

    #[test]
    fn dollars_to_cents_basic() {
        assert_eq!(dollars_to_cents(142.30), 14230);
        assert_eq!(dollars_to_cents(0.99), 99);
        assert_eq!(dollars_to_cents(0.0), 0);
        assert_eq!(dollars_to_cents(1000.00), 100000);
    }

    #[test]
    fn dollars_to_cents_rounding() {
        // 19.99 * 100.0 = 1998.9999... → rounds to 1999
        assert_eq!(dollars_to_cents(19.99), 1999);
        // 0.1 + 0.2 in float world → still rounds correctly
        assert_eq!(dollars_to_cents(0.1 + 0.2), 30);
    }

    #[test]
    fn format_dollars_basic() {
        assert_eq!(format_dollars(14230), "$142.30");
        assert_eq!(format_dollars(99), "$0.99");
        assert_eq!(format_dollars(0), "$0.00");
        assert_eq!(format_dollars(100000), "$1000.00");
    }

    #[test]
    fn format_dollars_negative() {
        assert_eq!(format_dollars(-500), "-$5.00");
    }

    // ── Month parsing ───────────────────────────────────────────

    #[test]
    fn parse_month_valid() {
        assert_eq!(parse_month(Some("2026-04")), (2026, 4));
        assert_eq!(parse_month(Some("2025-12")), (2025, 12));
    }

    #[test]
    fn parse_month_invalid_falls_back() {
        let now = Utc::now();
        assert_eq!(parse_month(Some("not-a-date")), (now.year(), now.month()));
        assert_eq!(parse_month(Some("2026-13")), (now.year(), now.month())); // month > 12
        assert_eq!(parse_month(None), (now.year(), now.month()));
    }

    // ── Financial email detection ───────────────────────────────

    #[test]
    fn is_financial_by_subject() {
        assert!(is_likely_financial("Your electric bill is ready", "noreply@utility.com"));
        assert!(is_likely_financial("Payment receipt for order #123", "shop@example.com"));
        assert!(is_likely_financial("Monthly statement available", "bank@example.com"));
        assert!(is_likely_financial("Invoice #INV-2026-04", "billing@vendor.com"));
    }

    #[test]
    fn is_financial_by_sender() {
        assert!(is_likely_financial("Hello", "billing@company.com"));
        assert!(is_likely_financial("Update", "noreply@paypal.com"));
        assert!(is_likely_financial("Alert", "alerts@chase.com"));
    }

    #[test]
    fn not_financial() {
        assert!(!is_likely_financial("Meeting tomorrow", "colleague@work.com"));
        assert!(!is_likely_financial("Project update", "manager@company.com"));
        assert!(!is_likely_financial("Happy birthday!", "friend@personal.com"));
    }

    // ── Store round-trips ───────────────────────────────────────

    fn test_store() -> (EncryptedStore, MasterKey, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("aivyx-finance-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = EncryptedStore::open(dir.join("store.db")).unwrap();
        let key = aivyx_crypto::derive_domain_key(&aivyx_crypto::MasterKey::generate(), b"finance");
        (store, key, dir)
    }

    #[test]
    fn save_and_load_transaction() {
        let (store, key, dir) = test_store();
        let tx = Transaction {
            id: "tx-1".into(),
            kind: TransactionKind::Expense,
            amount_cents: 1499,
            currency: "USD".into(),
            description: "Coffee".into(),
            category: Some("dining".into()),
            vendor: Some("Starbucks".into()),
            due_date: None,
            transaction_date: Utc::now(),
            source: TransactionSource::Manual,
            email_message_id: None,
            receipt_path: None,
            paid: false,
            confirmed: true,
            tags: vec![],
            created_at: Utc::now(),
        };

        save_transaction(&store, &key, &tx).unwrap();
        let loaded = load_all_transactions(&store, &key).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "tx-1");
        assert_eq!(loaded[0].amount_cents, 1499);
        assert_eq!(loaded[0].category, Some("dining".into()));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn load_transactions_filters_by_month() {
        let (store, key, dir) = test_store();

        // Transaction in April 2026
        let tx1 = Transaction {
            id: "tx-apr".into(),
            kind: TransactionKind::Expense,
            amount_cents: 5000,
            currency: "USD".into(),
            description: "April expense".into(),
            category: None,
            vendor: None,
            due_date: None,
            transaction_date: chrono::NaiveDate::from_ymd_opt(2026, 4, 15)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .map(|dt| DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc))
                .unwrap(),
            source: TransactionSource::Manual,
            email_message_id: None,
            receipt_path: None,
            paid: false,
            confirmed: true,
            tags: vec![],
            created_at: Utc::now(),
        };

        // Transaction in March 2026
        let tx2 = Transaction {
            id: "tx-mar".into(),
            transaction_date: chrono::NaiveDate::from_ymd_opt(2026, 3, 10)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .map(|dt| DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc))
                .unwrap(),
            ..tx1.clone()
        };

        save_transaction(&store, &key, &tx1).unwrap();
        save_transaction(&store, &key, &tx2).unwrap();

        let april = load_transactions_for_month(&store, &key, 2026, 4).unwrap();
        assert_eq!(april.len(), 1);
        assert_eq!(april[0].id, "tx-apr");

        let march = load_transactions_for_month(&store, &key, 2026, 3).unwrap();
        assert_eq!(march.len(), 1);
        assert_eq!(march[0].id, "tx-mar");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn save_and_load_budget_rule() {
        let (store, key, dir) = test_store();
        let rule = BudgetRule {
            id: "budget-1".into(),
            category: "dining".into(),
            monthly_limit_cents: 50000,
            created_at: Utc::now(),
        };

        save_budget_rule(&store, &key, &rule).unwrap();
        let rules = load_budget_rules(&store, &key).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].category, "dining");
        assert_eq!(rules[0].monthly_limit_cents, 50000);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn scan_cursor_round_trip() {
        let (store, key, dir) = test_store();

        assert_eq!(load_scan_cursor(&store, &key).unwrap(), 0);
        save_scan_cursor(&store, &key, 42).unwrap();
        assert_eq!(load_scan_cursor(&store, &key).unwrap(), 42);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn delete_transaction_works() {
        let (store, key, dir) = test_store();
        let tx = Transaction {
            id: "tx-del".into(),
            kind: TransactionKind::Expense,
            amount_cents: 100,
            currency: "USD".into(),
            description: "To delete".into(),
            category: None,
            vendor: None,
            due_date: None,
            transaction_date: Utc::now(),
            source: TransactionSource::Manual,
            email_message_id: None,
            receipt_path: None,
            paid: false,
            confirmed: true,
            tags: vec![],
            created_at: Utc::now(),
        };

        save_transaction(&store, &key, &tx).unwrap();
        assert_eq!(load_all_transactions(&store, &key).unwrap().len(), 1);

        delete_transaction(&store, "tx-del").unwrap();
        assert_eq!(load_all_transactions(&store, &key).unwrap().len(), 0);

        let _ = std::fs::remove_dir_all(dir);
    }

    // ── Upcoming bills ──────────────────────────────────────────

    #[test]
    fn upcoming_bills_filters_correctly() {
        let (store, key, dir) = test_store();

        // Bill due in 3 days (should appear)
        let bill_soon = Transaction {
            id: "bill-soon".into(),
            kind: TransactionKind::Bill,
            amount_cents: 14230,
            currency: "USD".into(),
            description: "Electric bill".into(),
            category: Some("utilities".into()),
            vendor: Some("PG&E".into()),
            due_date: Some(Utc::now() + chrono::Duration::days(3)),
            transaction_date: Utc::now(),
            source: TransactionSource::EmailDetection,
            email_message_id: None,
            receipt_path: None,
            paid: false,
            confirmed: true,
            tags: vec![],
            created_at: Utc::now(),
        };

        // Bill already paid (should NOT appear)
        let bill_paid = Transaction {
            id: "bill-paid".into(),
            paid: true,
            ..bill_soon.clone()
        };

        // Expense (should NOT appear — not a bill)
        let expense = Transaction {
            id: "expense-1".into(),
            kind: TransactionKind::Expense,
            due_date: None,
            ..bill_soon.clone()
        };

        save_transaction(&store, &key, &bill_soon).unwrap();
        save_transaction(&store, &key, &bill_paid).unwrap();
        save_transaction(&store, &key, &expense).unwrap();

        let bills = upcoming_bills(&store, &key, 7).unwrap();
        assert_eq!(bills.len(), 1);
        assert_eq!(bills[0].id, "bill-soon");

        let _ = std::fs::remove_dir_all(dir);
    }

    // ── Over budget detection ───────────────────────────────────

    #[test]
    fn over_budget_detects_overspend() {
        let (store, key, dir) = test_store();

        let now = Utc::now();
        // Expense this month: $600 in dining
        let tx = Transaction {
            id: "tx-over".into(),
            kind: TransactionKind::Expense,
            amount_cents: 60000,
            currency: "USD".into(),
            description: "Fancy dinners".into(),
            category: Some("dining".into()),
            vendor: None,
            due_date: None,
            transaction_date: now,
            source: TransactionSource::Manual,
            email_message_id: None,
            receipt_path: None,
            paid: false,
            confirmed: true,
            tags: vec![],
            created_at: now,
        };
        save_transaction(&store, &key, &tx).unwrap();

        // Budget: $500 for dining
        let rule = BudgetRule {
            id: "budget-dining".into(),
            category: "dining".into(),
            monthly_limit_cents: 50000,
            created_at: now,
        };
        save_budget_rule(&store, &key, &rule).unwrap();

        let over = over_budget_categories(&store, &key).unwrap();
        assert_eq!(over.len(), 1);
        assert_eq!(over[0].0, "dining");
        assert_eq!(over[0].1, 60000); // spent
        assert_eq!(over[0].2, 50000); // limit

        let _ = std::fs::remove_dir_all(dir);
    }

    // ── Tool schema tests ───────────────────────────────────────

    #[test]
    fn add_transaction_schema() {
        let (store, key, dir) = test_store();
        let tool = AddTransaction {
            store: Arc::new(store),
            key,
        };
        assert_eq!(tool.name(), "add_transaction");
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "kind"));
        assert!(required.iter().any(|v| v == "amount"));
        assert!(required.iter().any(|v| v == "description"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn list_transactions_schema() {
        let (store, key, dir) = test_store();
        let tool = ListTransactions {
            store: Arc::new(store),
            key,
        };
        assert_eq!(tool.name(), "list_transactions");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn budget_summary_schema() {
        let (store, key, dir) = test_store();
        let tool = BudgetSummary {
            store: Arc::new(store),
            key,
        };
        assert_eq!(tool.name(), "budget_summary");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn set_budget_schema() {
        let (store, key, dir) = test_store();
        let tool = SetBudget {
            store: Arc::new(store),
            key,
        };
        assert_eq!(tool.name(), "set_budget");
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "category"));
        assert!(required.iter().any(|v| v == "monthly_limit"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn mark_bill_paid_schema() {
        let (store, key, dir) = test_store();
        let tool = MarkBillPaid {
            store: Arc::new(store),
            key,
        };
        assert_eq!(tool.name(), "mark_bill_paid");

        let _ = std::fs::remove_dir_all(dir);
    }

    // ── Add + list round-trip via execute ────────────────────────

    #[tokio::test]
    async fn add_and_list_transaction() {
        let (store, _key, dir) = test_store();
        let store = Arc::new(store);

        // Use a shared master key so both tools derive the same finance key.
        let master = aivyx_crypto::MasterKey::generate();
        let fkey = aivyx_crypto::derive_domain_key(&master, b"finance");
        let fkey2 = aivyx_crypto::derive_domain_key(&master, b"finance");

        let add_tool = AddTransaction {
            store: Arc::clone(&store),
            key: fkey,
        };

        let res = add_tool.execute(serde_json::json!({
            "kind": "expense",
            "amount": 42.50,
            "description": "Lunch with Sarah",
            "category": "dining",
            "vendor": "The Grill"
        })).await.unwrap();

        assert_eq!(res["status"], "recorded");
        assert_eq!(res["amount"], "$42.50");

        let list_tool = ListTransactions {
            store: Arc::clone(&store),
            key: fkey2,
        };
        let list_res = list_tool.execute(serde_json::json!({})).await.unwrap();
        assert_eq!(list_res["count"], 1);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn delete_transaction_schema() {
        let dir = std::env::temp_dir().join(format!("aivyx-fin-del-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Arc::new(aivyx_crypto::EncryptedStore::open(dir.join("store.db")).unwrap());
        let key = aivyx_crypto::MasterKey::generate();
        let tool = DeleteTransactionAction {
            store,
            key: aivyx_crypto::derive_domain_key(&key, b"finance"),
        };
        assert_eq!(tool.name(), "delete_transaction");
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "id"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn update_transaction_schema() {
        let dir = std::env::temp_dir().join(format!("aivyx-fin-upd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Arc::new(aivyx_crypto::EncryptedStore::open(dir.join("store.db")).unwrap());
        let key = aivyx_crypto::MasterKey::generate();
        let tool = UpdateTransaction {
            store,
            key: aivyx_crypto::derive_domain_key(&key, b"finance"),
        };
        assert_eq!(tool.name(), "update_transaction");
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "id"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn delete_budget_schema() {
        let dir = std::env::temp_dir().join(format!("aivyx-fin-bud-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Arc::new(aivyx_crypto::EncryptedStore::open(dir.join("store.db")).unwrap());
        let tool = DeleteBudget { store };
        assert_eq!(tool.name(), "delete_budget");
        let schema = tool.input_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "id"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn delete_transaction_round_trip() {
        let dir = std::env::temp_dir().join(format!("aivyx-fin-rt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Arc::new(aivyx_crypto::EncryptedStore::open(dir.join("store.db")).unwrap());
        let master = aivyx_crypto::MasterKey::generate();
        let fkey = aivyx_crypto::derive_domain_key(&master, b"finance");

        // Add a transaction
        let add_tool = AddTransaction {
            store: Arc::clone(&store),
            key: aivyx_crypto::derive_domain_key(&master, b"finance"),
        };
        let res = add_tool.execute(serde_json::json!({
            "kind": "expense",
            "amount": 10.00,
            "description": "Test transaction",
        })).await.unwrap();
        let id = res["id"].as_str().unwrap().to_string();

        // Verify it exists
        let all = load_all_transactions(&store, &fkey).unwrap();
        assert_eq!(all.len(), 1);

        // Delete it
        let del_tool = DeleteTransactionAction {
            store: Arc::clone(&store),
            key: aivyx_crypto::derive_domain_key(&master, b"finance"),
        };
        let result = del_tool.execute(serde_json::json!({ "id": id })).await.unwrap();
        assert_eq!(result["status"], "deleted");

        // Verify it's gone
        let all = load_all_transactions(&store, &aivyx_crypto::derive_domain_key(&master, b"finance")).unwrap();
        assert!(all.is_empty());

        let _ = std::fs::remove_dir_all(dir);
    }
}
