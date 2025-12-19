## Obtain an Engine Binary

You can obtain an engine binary by cloning the repository and building a stable commit locally:

```bash
cargo build --release
```

## Launch the Binary

Launching the binary will populate:

```
%AppData%/Pulsar
```

with the default engine themes and configuration, then bring you directly to the **Pulsar Launcher**.

> [!NOTE]
> Your engine launcher may start in **light mode** by default. **DON’T PANIC!**
> To avoid attracting bugs, press `Ctrl + ,` to open the Settings window. From there, you can change the theme at the top to **Default Dark** (or any other theme you prefer).
>
> **Be sure to click `Save`**, then re-launch the engine to ensure all windows update correctly.

> [!WARNING]
> Buttons and features outside the scope of this guide may not work yet. Many parts of the engine are still **very much a WIP**.

<img width="1280" height="720" alt="Pulsar Launcher" src="https://github.com/user-attachments/assets/9e5e92eb-6e11-490b-bf56-2878c509ae78" />

---

## Navigate to Templates

From the launcher, navigate to the **Templates** section.

<img width="1280" height="720" alt="Templates View" src="https://github.com/user-attachments/assets/0953d513-2a8a-4d44-ae81-eb401f29a00c" />

---

## Select `Blank Project`

Choose **Blank Project**. The engine will prompt you to select a directory where the template will be cloned.

Once cloning completes, you’ll be asked to configure a new Git remote for pushing changes. The engine will store this as your default remote.

For now, you can safely skip this step due to the limitation below.

> [!NOTE]
> Pulsar stores most assets as **text** and integrates deeply with **Git** throughout the workflow. Collaboration is a first-class goal, and Git is the engine’s primary version control system.

> [!WARNING]
> There is currently a known issue on some platforms where **text input fields may not function correctly**.
> Fixes for this are planned as part of the upcoming **graphics pipeline overhaul**.

<img width="1280" height="720" alt="Project Creation" src="https://github.com/user-attachments/assets/cb82230c-df44-4e1e-9c19-6253255a1bf6" />

---

## The Editor

🎉 **Congrats — you made it!**
Your new project should open automatically. After a short delay, the 3D rendering engine will initialize and begin drawing the game scene.

<img width="1280" height="800" alt="Editor View" src="https://github.com/user-attachments/assets/8b173679-4821-4d90-9fee-78ec4e9254d8" />

---

## The File Drawer

Most navigation happens through the **File Drawer** in the bottom-left corner of the editor. From here, you can open and edit game assets directly.

<img width="1280" height="800" alt="File Drawer" src="https://github.com/user-attachments/assets/bc1efcb5-ccf7-4d7d-80fa-109b8a1a7cd1" />

---

## That’s All for Now!

Have fun exploring the different editors and browsing the project files on disk.
Welcome to **Pulsar** — and thanks for checking it out 💫
