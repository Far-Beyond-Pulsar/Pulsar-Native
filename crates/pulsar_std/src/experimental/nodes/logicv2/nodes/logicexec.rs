use crate::{blueprint, exec_output};

///
/// # Random Execute pin
///
/// Picks random pin to exec
///
/// (Do to limitations of the engine api currently this will only have 5 pins) later on as better exec pin handling it will be able to add on in engine.
///
#[blueprint(type: crate::NodeTypes::control_flow, category: "Logic Execution ++ (Experimental)",color="#861212ff")]
pub fn randexec() -> i32 {
    let selectedpin: i32 = rand::random_range(1..=5);

    match selectedpin{
        1=>exec_output!("A"),
        2=>exec_output!("B"),
        3=>exec_output!("C"),
        4=>exec_output!("D"),
        5=>exec_output!("E"),
        _=>panic!("random exec caught a impossible exec value, 1-5 went out of bounds, (Something MUST have FAILED!!!!)")
    }

    return selectedpin;
}
