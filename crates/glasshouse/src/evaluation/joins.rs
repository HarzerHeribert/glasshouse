use rusqlite::params;

use crate::routing::evidence::{
    MIN_SAMPLE_FOR_SUMMARY, RouteResponsiveness, RoutingObservation, row_to_observation,
};

use super::{EvaluationError, EvaluationKind, EvaluationObservations, UNKNOWN_COST_CLASS, sql_err};

/// Map line 1845's other five quantities — kept in its own block, practice
/// §77, so it cannot land on the same lines as another worker's.
///
/// # Why this reads `routing_observations` directly
///
/// `usable tool calls`, `repair loops`, `effective TTFC` and `reliability`
/// are all facts this ledger's own `route_outcomes_by_pairing_class`
/// (map line 1845's task-success half, above) has no column for — they live
/// on `crate::routing::evidence::RoutingObservation`, in the same physical
/// database file (`crate::database::open`, this struct's own doc comment on
/// [`Self::open`]) but a different table. The register's note stood
/// (`docs/product/evidence/phase-51.md`, *"three producers, not a join"*)
/// until they landed; this is the join, by `session_id`, exactly the shape
/// [`Self::route_outcomes_by_pairing_class`] already uses to reach
/// `sessions.pairing_class` from an evaluation row.
impl EvaluationObservations {
    /// One [`PairingClassResponsiveness`] per pairing class in `[from, to]`
    /// — the five quantities map line 1845 asks for beside task success.
    pub fn pairing_class_responsiveness(
        &self,
        from: i64,
        to: i64,
    ) -> Result<Vec<PairingClassResponsiveness>, EvaluationError> {
        self.refuse_unretained_window(from)?;

        // Half one: every routing-observation row a session in this project
        // recorded in the window, with the session's own pairing class —
        // `usable tool calls`, `repair loops`, `effective TTFC` and
        // `reliability` are all read from this set.
        let rows: Vec<(RoutingObservation, Option<String>)> = {
            let conn = self.lock();
            let mut statement = conn
                .prepare(
                    "SELECT r.*, s.pairing_class AS pairing_class
                       FROM routing_observations AS r
                       JOIN sessions AS s
                         ON s.id = r.session_id AND s.project_id = ?1
                      WHERE r.project_id = ?1 AND r.session_id IS NOT NULL
                        AND r.observed_at >= ?2 AND r.observed_at <= ?3",
                )
                .map_err(sql_err("read routing observations by pairing class"))?;
            let mapped = statement
                .query_map(params![self.project_id, from, to], |row| {
                    let pairing_class: Option<String> = row.get("pairing_class")?;
                    Ok((row_to_observation(row)?, pairing_class))
                })
                .map_err(sql_err("read routing observations by pairing class"))?;
            let mut rows = Vec::new();
            for row in mapped {
                let (observation, pairing_class) =
                    row.map_err(sql_err("read one routing observation by pairing class"))?;
                rows.push((
                    observation.map_err(|err| {
                        sql_err("decode one routing observation by pairing class")(
                            rusqlite::Error::ToSqlConversionFailure(Box::new(err)),
                        )
                    })?,
                    pairing_class,
                ));
            }
            rows
        };

        let mut by_class: std::collections::BTreeMap<String, Vec<RoutingObservation>> =
            std::collections::BTreeMap::new();
        for (observation, pairing_class) in rows {
            by_class
                .entry(pairing_class.unwrap_or_else(|| UNKNOWN_COST_CLASS.to_owned()))
                .or_default()
                .push(observation);
        }

        // Half two: this project's decisions per class (the same `decision`
        // count [`Self::route_outcomes_by_pairing_class`] reports as
        // `sessions`) and, of those, how many carry an overridden
        // [`EvaluationKind::RoutingOverrideDecided`] row — map line 1845's
        // `user overrides`.
        let (decisions, overridden): (
            std::collections::BTreeMap<String, i64>,
            std::collections::BTreeMap<String, i64>,
        ) = {
            let conn = self.lock();
            let mut statement = conn
                .prepare(
                    "WITH decision AS (
                         SELECT session_id AS session_id, MAX(seq) AS seq
                           FROM evaluation_observations
                          WHERE kind = ?1
                            AND session_id IS NOT NULL
                            AND observed_at >= ?2
                            AND observed_at <= ?3
                          GROUP BY session_id
                     ),
                     overridden AS (
                         SELECT DISTINCT session_id
                           FROM evaluation_observations
                          WHERE kind = ?4 AND subject = ?5
                            AND session_id IS NOT NULL
                            AND observed_at >= ?2
                            AND observed_at <= ?3
                     )
                     SELECT COALESCE(s.pairing_class, ?6),
                            COUNT(*),
                            SUM(CASE WHEN o.session_id IS NOT NULL THEN 1 ELSE 0 END)
                       FROM decision AS d
                       LEFT JOIN sessions AS s
                              ON s.id = d.session_id AND s.project_id = ?7
                       LEFT JOIN overridden AS o ON o.session_id = d.session_id
                      GROUP BY COALESCE(s.pairing_class, ?6)",
                )
                .map_err(sql_err("read decisions and overrides by pairing class"))?;
            let rows = statement
                .query_map(
                    params![
                        EvaluationKind::RoutingCostClassObserved.as_str(),
                        from,
                        to,
                        EvaluationKind::RoutingOverrideDecided.as_str(),
                        "overridden",
                        UNKNOWN_COST_CLASS,
                        self.project_id,
                    ],
                    |row| {
                        let bucket: String = row.get(0)?;
                        let decisions: i64 = row.get(1)?;
                        let overridden: i64 = row.get(2)?;
                        Ok((bucket, decisions, overridden))
                    },
                )
                .map_err(sql_err("read decisions and overrides by pairing class"))?;
            let mut decisions_by_class = std::collections::BTreeMap::new();
            let mut overridden_by_class = std::collections::BTreeMap::new();
            for row in rows {
                let (bucket, decisions, overridden) =
                    row.map_err(sql_err("read one pairing class's decisions and overrides"))?;
                decisions_by_class.insert(bucket.clone(), decisions);
                overridden_by_class.insert(bucket, overridden);
            }
            (decisions_by_class, overridden_by_class)
        };

        // Every bucket either half named — a class with rows but no
        // decision in this exact window (or the reverse) still gets a line,
        // honestly zero on the side it has nothing for, rather than being
        // dropped.
        let mut buckets: std::collections::BTreeSet<String> = by_class.keys().cloned().collect();
        buckets.extend(decisions.keys().cloned());

        Ok(buckets
            .into_iter()
            .map(|bucket| {
                let group = by_class.get(&bucket).cloned().unwrap_or_default();

                let mut tool_rounds_recorded = 0usize;
                let mut tool_rounds_positive = 0usize;
                let mut repairs_sum: i64 = 0;
                let mut repairs_sample = 0usize;
                for observation in &group {
                    if let Some(rounds) = observation.tool_rounds {
                        tool_rounds_recorded += 1;
                        if rounds > 0 {
                            tool_rounds_positive += 1;
                        }
                    }
                    if let Some(repairs) = observation.repairs {
                        repairs_sum += repairs;
                        repairs_sample += 1;
                    }
                }
                let usable_tool_calls = (tool_rounds_recorded >= MIN_SAMPLE_FOR_SUMMARY)
                    .then(|| tool_rounds_positive as f64 / tool_rounds_recorded as f64);
                let repair_loops = (repairs_sample >= MIN_SAMPLE_FOR_SUMMARY)
                    .then(|| repairs_sum as f64 / repairs_sample as f64);

                let responsiveness = RouteResponsiveness::from_observations(&group);
                let reliability = responsiveness.failure_rate.map(|p| 1.0 - p);

                let class_decisions = decisions.get(&bucket).copied().unwrap_or(0);
                let class_overridden = overridden.get(&bucket).copied().unwrap_or(0);
                let user_overrides = (class_decisions as usize >= MIN_SAMPLE_FOR_SUMMARY)
                    .then(|| class_overridden as f64 / class_decisions as f64);

                PairingClassResponsiveness {
                    bucket,
                    decisions: class_decisions,
                    usable_tool_calls,
                    usable_tool_calls_sample: tool_rounds_recorded,
                    repair_loops,
                    repair_loops_sample: repairs_sample,
                    effective_ttfc_ms: responsiveness.effective_ttfc_ms(),
                    effective_ttfc_sample: responsiveness.raw_ttfc_sample,
                    reliability,
                    reliability_sample: responsiveness.failure_rate_sample,
                    user_overrides,
                    user_overrides_sample: class_decisions,
                }
            })
            .collect())
    }
}

/// [`EvaluationObservations::pairing_class_responsiveness`]'s result — map
/// line 1845's other five quantities, one row per pairing class. Every
/// figure carries its own sample count and is `None` — *not enough* — below
/// [`MIN_SAMPLE_FOR_SUMMARY`], the same floor
/// [`crate::evaluation::RouteOutcomeCounts::reported_turns`]'s own task-success half sits
/// behind through [`RouteResponsiveness`].
#[derive(Debug, Clone, PartialEq)]
pub struct PairingClassResponsiveness {
    pub bucket: String,
    /// This class's routed sessions in the window — the same count
    /// [`EvaluationObservations::route_outcomes_by_pairing_class`] reports
    /// as `sessions`, and [`Self::user_overrides`]'s own denominator.
    pub decisions: i64,
    pub usable_tool_calls: Option<f64>,
    pub usable_tool_calls_sample: usize,
    pub repair_loops: Option<f64>,
    pub repair_loops_sample: usize,
    pub effective_ttfc_ms: Option<f64>,
    pub effective_ttfc_sample: usize,
    pub reliability: Option<f64>,
    pub reliability_sample: usize,
    pub user_overrides: Option<f64>,
    pub user_overrides_sample: i64,
}
