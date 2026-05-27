use crate::{blueprint,NodeTypes,exec_output};

static CATAGORY:&str = "ClassyPallet";
static COLOR:&str = "#0e0ed86e";

#[blueprint(type:NodeTypes::control_flow,category:CATAGORY,color:COLOR)]
pub fn random_exec_switch() {
    let state = rand::random_range(1..=2);
    if state==1{
        exec_output!("ONE");
    } else {
        exec_output!("TWO");
    }
}

#[blueprint(type:pure,category:CATAGORY,color:COLOR)]
pub fn Var1(){
    "Random - 1".to_string();
}

#[blueprint(type:pure,category:CATAGORY,color:COLOR)]
pub fn Var2(){
    "Random - 2".to_string();
}