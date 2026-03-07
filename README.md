# Agentis

**Agentis** is not just another programming language. It is a system where the **programming language and the Version Control System (VCS) are fused into a single identity.**

In the traditional world, code is plain text and Git tries to track it. In Agentis, there are no text files. Code is a binary, hashed Directed Acyclic Graph (DAG), stored directly in the `.agentis/objects/` directory.

### Why Agentis?
* **Zero-Dependency Rust:** No SQLite, no Tokio, no bulky frameworks. Pure, raw Rust (only `sha2` for integrity).
* **AI-Native:** Designed for agents. Instead of merge conflicts, we use semantic branching (`explore` blocks).
* **Cognitive Budget (CB):** Every operation costs "fuel". Agentis kills infinite loops before they eat your CPU.
* **Genesis-First:** Forget `main`. Everything begins with the `genesis` branch.

---

## Architecture (Hardcore Vanilla)

Agentis works as a hybrid between a compiler and Git internals:
1. **Lexer/Parser:** Transforms code into an Abstract Syntax Tree (AST).
2. **Hashing:** Every AST node gets a unique SHA-256 hash.
3. **Storage:** Nodes are saved as binary objects (content-addressable storage).
4. **Interpreter:** Executes the AST directly while enforcing the **Cognitive Budget (CB)**.
5. **P2P Sync:** Code synchronization happens over raw TCP sockets.

## Cognitive Budget (CB)
To prevent hallucinating agents from bringing down the system, we use **CB**.
* Math operations: 1 CB
* Function calls: 5 CB
* Memory allocation: Dynamic based on size

Once the budget hits zero, the system raises a `CognitiveOverload` and safely terminates the execution branch.

## Getting Started

```bash
# Initialize a new repository and the genesis branch
agentis init

# Execute code from a specific branch
agentis run genesis
```

---

> "Agentis uses text files today only so it can eliminate them forever tomorrow."
