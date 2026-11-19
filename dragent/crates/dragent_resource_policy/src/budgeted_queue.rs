use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Eviction<T> {
    pub items: Vec<T>,
    pub weight: usize,
}

/// FIFO hot-content window bounded by both item count and caller-defined
/// weight. Evicted values can be persisted as a cold tier by the caller.
pub struct BudgetedQueue<T> {
    entries: VecDeque<(T, usize)>,
    max_items: usize,
    max_weight: usize,
    weight: usize,
}

impl<T> BudgetedQueue<T> {
    pub fn new(max_items: usize, max_weight: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            max_items,
            max_weight,
            weight: 0,
        }
    }

    pub fn push(&mut self, value: T, weight: usize) -> Eviction<T> {
        self.weight = self.weight.saturating_add(weight);
        self.entries.push_back((value, weight));
        self.enforce_budget()
    }

    pub fn set_budget(&mut self, max_items: usize, max_weight: usize) -> Eviction<T> {
        self.max_items = max_items;
        self.max_weight = max_weight;
        self.enforce_budget()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn weight(&self) -> usize {
        self.weight
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.entries.iter().map(|(value, _)| value)
    }

    fn enforce_budget(&mut self) -> Eviction<T> {
        let mut items = Vec::new();
        let mut evicted_weight = 0usize;
        while self.entries.len() > self.max_items || self.weight > self.max_weight {
            let Some((value, weight)) = self.entries.pop_front() else {
                break;
            };
            self.weight = self.weight.saturating_sub(weight);
            evicted_weight = evicted_weight.saturating_add(weight);
            items.push(value);
        }
        Eviction {
            items,
            weight: evicted_weight,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_oldest_by_item_budget() {
        let mut queue = BudgetedQueue::new(2, usize::MAX);
        queue.push("one", 1);
        queue.push("two", 1);
        let evicted = queue.push("three", 1);
        assert_eq!(evicted.items, vec!["one"]);
        assert_eq!(
            queue.iter().copied().collect::<Vec<_>>(),
            vec!["two", "three"]
        );
    }

    #[test]
    fn evicts_until_weight_is_bounded() {
        let mut queue = BudgetedQueue::new(10, 5);
        queue.push("one", 3);
        let evicted = queue.push("two", 4);
        assert_eq!(evicted.items, vec!["one"]);
        assert_eq!(queue.weight(), 4);
    }
}
