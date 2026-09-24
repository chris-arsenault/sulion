//! Calls, their results, and the runtime evidence folded into them.

use crate::ingest::canonical::Block;

use super::*;

impl<B: Backend> Reducer<B> {
    fn remember_op(&mut self, op: OpRow) {
        let key = op.key();
        if self.ops.contains_key(&key) {
            return;
        }
        self.ops_by_pair
            .entry(op.pair_id.clone())
            .or_default()
            .insert(key);
        self.ops.insert(key, op);
    }

    /// Store a changed operation: a new call, or an update to a stored one.
    fn put_op(&mut self, op: OpRow, input_changed: bool) {
        let key = op.key();
        self.ops_by_pair
            .entry(op.pair_id.clone())
            .or_default()
            .insert(key);
        self.ops.insert(key, op);
        if !self.new_ops.contains(&key) {
            self.updated_ops.insert(key);
            if input_changed {
                self.input_changed.insert(key);
            }
        }
    }

    /// Operations with `pair_id` called before `before`, oldest first.
    async fn ops_for_pair(&mut self, pair_id: &str, before: i64) -> anyhow::Result<Vec<OpRow>> {
        if self.queried_pairs.insert(pair_id.to_string()) {
            for op in self.backend.ops_by_pair(pair_id, before).await? {
                self.remember_op(op);
            }
        }
        let mut found: Vec<OpRow> = self
            .ops_by_pair
            .get(pair_id)
            .into_iter()
            .flatten()
            .filter_map(|key| self.ops.get(key))
            .filter(|op| op.call_offset < before)
            .cloned()
            .collect();
        found.sort_by_key(|op| op.call_offset);
        Ok(found)
    }

    /// The latest call with this id before the result, in any turn.
    async fn latest_op_for_pair(
        &mut self,
        pair_id: &str,
        before: i64,
    ) -> anyhow::Result<Option<OpRow>> {
        let local = self
            .ops_by_pair
            .get(pair_id)
            .into_iter()
            .flatten()
            .filter_map(|key| self.ops.get(key))
            .filter(|op| op.call_offset < before)
            .max_by_key(|op| op.call_offset)
            .cloned();
        // A call made in this batch is newer than anything persisted.
        if local
            .as_ref()
            .is_some_and(|op| op.call_offset > self.batch_start)
        {
            return Ok(local);
        }
        Ok(self.ops_for_pair(pair_id, before).await?.pop())
    }

    pub(super) async fn apply_calls(
        &mut self,
        turn_id: i64,
        event: &StoredEvent,
    ) -> anyhow::Result<()> {
        for tool in tool_uses_in(event) {
            let turn = self.turn_mut(turn_id);
            turn.has_errors |= tool.is_error;
            let operation_ord = turn.operation_count;
            turn.operation_count += 1;
            let op = OpRow {
                turn_id,
                operation_ord,
                pair_id: tool.id.unwrap_or_else(|| format!("noid-{operation_ord}")),
                name: tool.name,
                raw_name: tool.raw_name,
                operation_type: tool.operation_type,
                category: tool.category,
                input: tool.input,
                result_content: None,
                result_payload: None,
                result_is_error: false,
                is_error: tool.is_error,
                is_pending: true,
                call_offset: event.byte_offset,
                call_at: event.timestamp,
                changed_at: event.byte_offset,
                call_error: tool.is_error,
                running_cell: None,
                finished_at: None,
            };
            self.new_ops.insert(op.key());
            self.put_op(op, true);
        }
        Ok(())
    }

    pub(super) async fn apply_results(
        &mut self,
        turn_id: i64,
        event: &StoredEvent,
    ) -> anyhow::Result<()> {
        for block in event
            .blocks
            .iter()
            .filter(|block| block.kind == BlockKind::ToolResult)
        {
            if block.is_error.unwrap_or(false) {
                self.turn_mut(turn_id).has_errors = true;
            }
            let Some(pair_id) = block.tool_id.as_deref() else {
                continue;
            };
            let Some(mut op) = self.latest_op_for_pair(pair_id, event.byte_offset).await? else {
                continue;
            };
            set_result(&mut op, block, event);
            self.close_waited_cells(&op, block, event).await?;
            let (owner, is_error) = (op.turn_id, op.is_error);
            self.put_op(op, false);
            if self.load_turn(owner).await? {
                self.turn_mut(owner).has_errors |= is_error;
            }
        }
        Ok(())
    }

    /// A completed `wait` ends the calls still running in the cell it waited
    /// on.
    async fn close_waited_cells(
        &mut self,
        wait: &OpRow,
        block: &Block,
        event: &StoredEvent,
    ) -> anyhow::Result<()> {
        if wait.tool_short_name() != Some("wait") || running_cell(block.text.as_deref()).is_some() {
            return Ok(());
        }
        let Some(cell) = wait
            .input
            .as_ref()
            .and_then(|input| input.get("cell_id"))
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            return Ok(());
        };
        for op in self.backend.ops_by_running_cell(&cell).await? {
            self.remember_op(op);
        }
        let running: Vec<OpRow> = self
            .ops
            .values()
            .filter(|op| {
                op.running_cell.as_deref() == Some(cell.as_str()) && op.key() != wait.key()
            })
            .cloned()
            .collect();
        for mut op in running {
            op.running_cell = None;
            op.finished_at = Some(event.timestamp);
            op.changed_at = event.byte_offset;
            self.put_op(op, false);
        }
        Ok(())
    }

    /// Runtime evidence folds into the one call it belongs to: the call with
    /// its id, else the single `exec` whose window encloses it. Evidence with
    /// no single owner stays on its own event as bookkeeping.
    pub(super) async fn correlate_runtime(
        &mut self,
        event: StoredEvent,
    ) -> anyhow::Result<StoredEvent> {
        let evidence: Vec<(Value, bool)> = event
            .blocks
            .iter()
            .filter_map(|block| {
                runtime_evidence(block)
                    .map(|value| (value.clone(), block.is_error.unwrap_or(false)))
            })
            .collect();
        if evidence.is_empty() {
            return Ok(event);
        }
        let mut output = event.clone();
        for (evidence, failed) in evidence {
            let item_id = evidence["runtime_item"]["id"].as_str().map(str::to_string);
            let started = evidence
                .get("started_at_ms")
                .and_then(Value::as_i64)
                .and_then(DateTime::<Utc>::from_timestamp_millis);
            let exact = match &item_id {
                Some(id) => self.ops_for_pair(id, event.byte_offset).await?,
                None => Vec::new(),
            };
            let candidates = if exact.is_empty() {
                for op in self
                    .backend
                    .exec_candidates(event.byte_offset, started)
                    .await?
                {
                    self.remember_op(op);
                }
                self.ops
                    .values()
                    .filter(|op| {
                        op.call_offset < event.byte_offset
                            && Some(op.pair_id.as_str()) != item_id.as_deref()
                            && op.encloses(started)
                    })
                    .cloned()
                    .collect()
            } else {
                exact
            };
            if candidates.len() != 1 {
                mark_uncorrelated(&mut output, &evidence);
                continue;
            }
            let mut op = candidates.into_iter().next().expect("one candidate");
            op.call_error |= failed;
            attach_to_input(&mut op.input, &evidence);
            if !op.is_pending {
                attach_to_output(&mut op.result_payload, &evidence);
                op.result_is_error |= failed;
            }
            op.is_error = op.call_error || op.result_is_error;
            op.changed_at = event.byte_offset;
            let (owner, is_error) = (op.turn_id, op.is_error);
            self.put_op(op, true);
            if self.load_turn(owner).await? {
                self.turn_mut(owner).has_errors |= is_error;
            }
            output.blocks.clear();
        }
        Ok(output)
    }
}

/// A result lands on its call; the last one wins. Evidence already folded
/// into the call belongs on the result too.
fn set_result(op: &mut OpRow, block: &Block, event: &StoredEvent) {
    op.result_content = block.text.clone();
    op.result_payload = block.tool_output.clone();
    if let Some(items) = op
        .input
        .as_ref()
        .and_then(|input| input.get("runtime_items"))
        .and_then(Value::as_array)
        .cloned()
    {
        for evidence in &items {
            attach_to_output(&mut op.result_payload, evidence);
        }
    }
    op.result_is_error = block.is_error.unwrap_or(false) || op.call_error;
    op.is_pending = false;
    op.is_error = op.call_error || op.result_is_error;
    op.changed_at = event.byte_offset;
    match running_cell(block.text.as_deref()) {
        Some(cell) => {
            op.running_cell = Some(cell.to_string());
            op.finished_at = None;
        }
        None => {
            op.running_cell = None;
            op.finished_at = Some(event.timestamp);
        }
    }
}
