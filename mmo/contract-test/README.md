# Orchestrator contract test

Compiles the game's own `OrchestratorClient.cs` (from the zone-hooks code) on its own and runs
it against a live orchestrator, so the C# side and the Rust side are checked against each other.

```sh
# start the orchestrator with world.example.json and a fresh database, then:
dotnet run -- http://127.0.0.1:8080 <ZONE_SECRET>
```

Uses only the .NET standard library; `nuget.config` clears package sources so it builds offline.
Expects the zones from `mmo/orchestrator/world.example.json` and an empty database.
