use super::QueryIterator;
use crate::error::Result;
use crate::sql::plan::Aggregate;
use crate::sql::types::Value;

use std::cmp::Ordering;
use std::collections::HashMap;

/// Aggregates rows (i.e. GROUP BY).
///
/// TODO: revisit this and clean it up.
pub(super) fn aggregate(
    mut source: QueryIterator,
    aggregates: Vec<Aggregate>,
) -> Result<QueryIterator> {
    let mut accumulators: HashMap<Vec<Value>, Vec<Box<dyn Accumulator>>> = HashMap::new();
    let agg_count = aggregates.len();
    while let Some(mut row) = source.next().transpose()? {
        accumulators
            .entry(row.split_off(aggregates.len()))
            .or_insert(aggregates.iter().map(<dyn Accumulator>::from).collect())
            .iter_mut()
            .zip(row)
            .try_for_each(|(acc, value)| acc.accumulate(&value))?
    }
    // If there were no rows and no group-by columns, return a row of empty accumulators:
    // SELECT COUNT(*) FROM t WHERE FALSE
    if accumulators.is_empty() && aggregates.len() == source.columns.len() {
        accumulators.insert(Vec::new(), aggregates.iter().map(<dyn Accumulator>::from).collect());
    }
    Ok(QueryIterator {
        columns: source
            .columns
            .into_iter()
            .enumerate()
            .map(|(i, c)| if i < agg_count { None } else { c })
            .collect(),
        rows: Box::new(accumulators.into_iter().map(|(bucket, accs)| {
            Ok(accs.into_iter().map(|acc| acc.aggregate()).chain(bucket).collect())
        })),
    })
}

// An accumulator
pub trait Accumulator: std::fmt::Debug + Send {
    // Accumulates a value
    fn accumulate(&mut self, value: &Value) -> Result<()>;

    // Calculates a final aggregate
    fn aggregate(&self) -> Value;
}

impl dyn Accumulator {
    fn from(aggregate: &Aggregate) -> Box<dyn Accumulator> {
        match aggregate {
            Aggregate::Average => Box::new(Average::new()),
            Aggregate::Count => Box::new(Count::new()),
            Aggregate::Max => Box::new(Max::new()),
            Aggregate::Min => Box::new(Min::new()),
            Aggregate::Sum => Box::new(Sum::new()),
        }
    }
}

// Count non-null values
#[derive(Debug)]
pub struct Count {
    count: u64,
}

impl Count {
    pub fn new() -> Self {
        Self { count: 0 }
    }
}

impl Accumulator for Count {
    fn accumulate(&mut self, value: &Value) -> Result<()> {
        match value {
            Value::Null => {}
            _ => self.count += 1,
        }
        Ok(())
    }

    fn aggregate(&self) -> Value {
        Value::Integer(self.count as i64)
    }
}

// Average value
#[derive(Debug)]
pub struct Average {
    count: Count,
    sum: Sum,
}

impl Average {
    pub fn new() -> Self {
        Self { count: Count::new(), sum: Sum::new() }
    }
}

impl Accumulator for Average {
    fn accumulate(&mut self, value: &Value) -> Result<()> {
        self.count.accumulate(value)?;
        self.sum.accumulate(value)?;
        Ok(())
    }

    fn aggregate(&self) -> Value {
        match (self.sum.aggregate(), self.count.aggregate()) {
            (Value::Integer(s), Value::Integer(c)) => Value::Integer(s / c),
            (Value::Float(s), Value::Integer(c)) => Value::Float(s / c as f64),
            _ => Value::Null,
        }
    }
}

// Maximum value
#[derive(Debug)]
pub struct Max {
    max: Option<Value>,
}

impl Max {
    pub fn new() -> Self {
        Self { max: None }
    }
}

impl Accumulator for Max {
    fn accumulate(&mut self, value: &Value) -> Result<()> {
        if let Some(max) = &mut self.max {
            match value.partial_cmp(max) {
                _ if max.datatype() != value.datatype() => *max = Value::Null,
                None => *max = Value::Null,
                Some(Ordering::Greater) => *max = value.clone(),
                Some(Ordering::Equal) | Some(Ordering::Less) => {}
            };
        } else {
            self.max = Some(value.clone())
        }
        Ok(())
    }

    fn aggregate(&self) -> Value {
        match &self.max {
            Some(value) => value.clone(),
            None => Value::Null,
        }
    }
}

// Minimum value
#[derive(Debug)]
pub struct Min {
    min: Option<Value>,
}

impl Min {
    pub fn new() -> Self {
        Self { min: None }
    }
}

impl Accumulator for Min {
    fn accumulate(&mut self, value: &Value) -> Result<()> {
        if let Some(min) = &mut self.min {
            match value.partial_cmp(min) {
                _ if min.datatype() != value.datatype() => *min = Value::Null,
                None => *min = Value::Null,
                Some(Ordering::Less) => *min = value.clone(),
                Some(Ordering::Equal) | Some(Ordering::Greater) => {}
            };
        } else {
            self.min = Some(value.clone())
        }
        Ok(())
    }

    fn aggregate(&self) -> Value {
        match &self.min {
            Some(value) => value.clone(),
            None => Value::Null,
        }
    }
}

// Sum of values
#[derive(Debug)]
pub struct Sum {
    sum: Option<Value>,
}

impl Sum {
    pub fn new() -> Self {
        Self { sum: None }
    }
}

impl Accumulator for Sum {
    fn accumulate(&mut self, value: &Value) -> Result<()> {
        self.sum = match (&self.sum, value) {
            (Some(Value::Integer(s)), Value::Integer(i)) => Some(Value::Integer(s + i)),
            (Some(Value::Float(s)), Value::Float(f)) => Some(Value::Float(s + f)),
            (None, Value::Integer(i)) => Some(Value::Integer(*i)),
            (None, Value::Float(f)) => Some(Value::Float(*f)),
            _ => Some(Value::Null),
        };
        Ok(())
    }

    fn aggregate(&self) -> Value {
        match &self.sum {
            Some(value) => value.clone(),
            None => Value::Null,
        }
    }
}