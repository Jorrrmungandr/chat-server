use fastrand;

mod adjectives;
mod animals;


pub fn random_name() -> String {
    let adjective = fastrand::choice(adjectives::ADJECTIVES).unwrap();
    let animal = fastrand::choice(animals::ANIMALS).unwrap();
    format!("{adjective}{animal}")
}

#[macro_export]
macro_rules! b {
    ($result:expr) => {
        match $result {
            Ok(ok) => ok,
            Err(err) => break Err(err.into()),
        }
    }
}