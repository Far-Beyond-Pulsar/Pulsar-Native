// PanelView promises it's all 3 multi-thread esq. traits HOLD
// pub trait PanelView: 'static + Send + Sync

// int do_thing(const int* n) {
//     // I promise I won't modify 'n'
// }

struct OtherThing {
    // Heap allocated - stack allocated (binary baked) str .data: 
    // 
    // -> pseudo-pointer to something being used in MyThing.some_field
    some_og_field: Box<&'static str>,
}

struct MyThing {
    some_field: Box<String>
}


impl EditorPlugin for MyThing {
    fn do_the_thing_with_someone_elses_data(&self, other_data: Box<Rc<&'a str>>) {
        // ... 
    }
}

let other = OtherThing { some_og_field: Box::new("wow") };
let mine = MyThing { some_field: Box::from(other.some_og_field.deref())}
let a = do_the_thing_with_someone_elses_data(other, mine)