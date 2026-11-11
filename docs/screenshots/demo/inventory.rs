use std::collections::HashMap;
use std::fmt;

/// An item on the shelf, with how many are left.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub name: String,
    pub price_cents: u32,
    pub quantity: u32,
}

impl fmt::Display for Item {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let dollars = self.price_cents as f64 / 100.0;
        write!(f, "{:<12} ${:>7.2}  x{}", self.name, dollars, self.quantity)
    }
}

pub struct Inventory {
    items: HashMap<String, Item>,
}

impl Inventory {
    pub fn new() -> Self {
        Self { items: HashMap::new() }
    }

    pub fn restock(&mut self, name: &str, price_cents: u32, quantity: u32) {
        let item = self.items.entry(name.to_owned()).or_insert(Item {
            name: name.to_owned(),
            price_cents,
            quantity: 0,
        });
        item.quantity += quantity;
    }

    pub fn sell(&mut self, name: &str) -> Option<u32> {
        let item = self.items.get_mut(name)?;
        if item.quantity == 0 {
            return None;
        }
        item.quantity -= 1;
        Some(item.price_cents)
    }

    pub fn total_value(&self) -> u64 {
        self.items
            .values()
            .map(|item| item.price_cents as u64 * item.quantity as u64)
            .sum()
    }
}

fn main() {
    let mut shop = Inventory::new();
    shop.restock("brush", 1250, 8);
    shop.restock("ink stick", 900, 12);
    shop.restock("paper", 300, 40);
    // 墨と筆: ink and brush
    println!("total: {} cents", shop.total_value());
}
