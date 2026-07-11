<!-- =============================== -->
<!--         PULSAR ENGINE           -->
<!-- =============================== -->

<p align="center">
  <a href="https://github.com/Far-Beyond-Pulsar/Pulsar-Native">
    <img width="160" height="160" alt="Pulsar Logo" src="https://github.com/user-attachments/assets/e36bfc3b-f5e8-437e-a50a-967ee254a0eb" style="background-color: transparent !important; border: none !important;" />
  </a>
</p>

<h1 align="center">Pulsar Engine</h1>

<p align="center">
  <strong>A next‑generation, experimental game engine focused on scalability, modularity, and modern systems design.</strong>
</p>
<img width="3840" height="1649" alt="deviceframes" src="https://github.com/user-attachments/assets/c850a05b-c863-4c1f-bd4d-4dd79c6b9fe1" />
<p align="center">
  Device Mockup created from <a href="https://deviceframes.com/templates/macbook-pro-m1-14">MacBook Pro mockups</a>
</p>

---

## ⚡ Beyond the Standard

Pulsar Engine is built by developers who believe core engine tech should be more than just a collection of legacy features. We are rebuilding the foundation of world simulation with clarity and intent.

*   **Scalable World Simulation:** Advanced spatial partitioning for high-fidelity environments.
*   **Rust-First Tooling:** Leveraging modern memory safety and performance from the ground up.
*   **Explicit Architecture:** Modular design that favors maintainability over short-term hacks.
*   **In-Editor Stability:** prioritizing a reliable workspace for creators as a first-class citizen.

---

## ⚠️ Project Status

> [!IMPORTANT]  
> **Pulsar is in early-stage development.** You likely cannot build a full production game yet. The codebase is experimental, subject to heavy architectural shifts, and open to community contributions.

**Current Roadmap:** [View Projects & Milestones](https://github.com/orgs/Far-Beyond-Pulsar/projects/1)

---

## 🤝 Join the Movement

Development happens in the open. Whether you're a systems engineer or a technical artist, your input helps shape the engine.

*   💬 **[Discord](https://discord.gg/5eAKDC53SX):** Real-time dev updates and brainstorming.
*   🧠 **[GitHub Discussions](https://github.com/Far-Beyond-Pulsar/Pulsar-Native/discussions):** Long-form design proposals.
*   ⭐ **Support:** Star, Watch, or Fork the repository to boost visibility.

---

## Instalaltion

Download the appropriate binary for your machine from the `Releases` page

## Source Build

The Pulsar-Native repo consists of many crates and git sub modules leading to a multi-step setup to build from source.

### Clone the repo

Cloning out repo gives you a full copy of the entire project hsitory locally right on your machine

```bash
git clone https://github.com/Far-Beyond-Pulsar/Pulsar-Native
```

### Pull the Submodules

Sub modules allow us to store code in other GitHub repos and reference them in the main repo helping to reduce repo bloat and enforce separation

```bash
git submodule update --init
```

### Install Deps (Linux)

On linux you may need to install some packages to allow the build to complete properly; here are the ones we needed on Ubuntu Desktop:

```bash
sudo apt update

sudo apt install -y \
  gcc g++ clang libfontconfig-dev libwayland-dev \
  libwebkit2gtk-4.1-dev libxkbcommon-x11-dev libx11-xcb-dev \
  libssl-dev libzstd-dev \
  vulkan-validationlayers libvulkan1
```

### Build via Cargo

```bash
cargo build --release
```

## 🛰️ Related Ecosystems

### **Horizon Game Server**
If you are interested in massive multiplayer scaling, check out **Horizon**, developed by many of the same contributors.

<a href="https://github.com/Far-Beyond-Dev/Horizon">
  <img src="https://github.com/user-attachments/assets/54a8e1a6-32ce-42fb-853f-04e38a52ae00" alt="Horizon Game Server" width="400" />
</a>

---

<p align="center">
  <em>Pulsar Engine developed by Tristan J. Poland and the Pulsar community.</em>
</p>
