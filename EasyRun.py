import subprocess
import keyboard
import psutil
import sys

Pulsar = None

CargoCommand="cargo run --release -p pulsar_engine -j 4"
ALTCargoCommand="cargo run -p pulsar_engine"

States = {"Altcommand":False,"Active":True,"DebugMode":True}

## Main Program - Handling Pulsar EXEC ##

def CheckActive():
    global States
    return not States["Active"]

def Start():
    global States
    global Pulsar
    Logtype=States["DebugMode"] and sys.stdout or subprocess.DEVNULL
    Command=States["Altcommand"] and ALTCargoCommand or CargoCommand
    Pulsar=subprocess.Popen(Command,shell=True,stdout=Logtype,stderr=Logtype)
    print(States["Altcommand"] and "Building Pulsar(Cargo) (Slow But Fast)" or "Building Pulsar(Cargo) (Fast But Slow)")
    return Pulsar

def StopCargo():
    if CheckActive(): return
    try:
        for proc in psutil.process_iter(['name']):
            if proc.info['name'] == "cargo.exe":
                proc.terminate()
    except:
        print("An error has occurred, cannot stop cargo, maybe its admin or not running?")

def Restart():
    if CheckActive(): return
    global Pulsar
    StopCargo()
    Start()
    return Pulsar

## Control Hooks ##

def EditState(packet):
    global States
    Token=packet[0]
    Value=packet[1]
    if Value=="@Switch":
        States[Token]=not States[Token]
    else:
        States[Token]=Value
    print(Token+" Has been Updated To: "+str(States[Token]))
    return States[Token]

def Hook_Keyboard():
    Controls={
        "ctrl+r":[Restart,"Restart Pulsar."],
        "alt+o":[StopCargo,"Close Pulsar."],
        "alt+g":[lambda: EditState(["Active","@Switch"]),"Toggles Easy-run Controls."],
        "alt+t":[lambda: EditState(["Altcommand","@Switch"]),"Toggles Fast Compile But Heavy performance loss."],
        "alt+d":[lambda: EditState(["DebugMode","@Switch"]),"Toggles Output for Pulsar Building."],
        "alt+Y":[lambda: print(States),"Dump Current States."]
    }
    print("---")
    print("Control Mappings:")
    for Key,ControlMapping in Controls.items():
        try:
            keyboard.add_hotkey(Key,ControlMapping[0])
            print(Key+":"+ControlMapping[1])
        except Exception as ControlError:
            print("Control Compile Error: "+str(ControlError))
    print("---")

## Main Starter ##
def Main():
    print("-- Easy Run --")
    Hook_Keyboard()
    Start()
    keyboard.wait("alt+c")
    print("Easy-Runner Signing off.")
    StopCargo()

if __name__ == "__main__":
    Main()