// Barotrauma MMO mod. This file is part of a modified version of Barotrauma;
// see docs/mmo/DESIGN.md. Not made or endorsed by Undertow Games.
#nullable enable

using System;
using System.Collections.Generic;
using System.Linq;
using System.Threading.Tasks;
using System.Xml.Linq;

namespace Barotrauma.Networking.Mmo
{
    /// <summary>
    /// Zone settings, read from environment variables. Zone mode is off (vanilla server) unless
    /// MMO_ORCHESTRATOR_URL is set.
    /// </summary>
    sealed record MmoZoneConfig(
        Uri OrchestratorUrl,
        string ZoneSecret,
        string ZoneKey,
        string PublicAddress,
        string? ExitEndZone,
        string? ExitStartZone,
        bool AllowUnauthenticated)
    {
        public static MmoZoneConfig? FromEnvironment()
        {
            string? url = Environment.GetEnvironmentVariable("MMO_ORCHESTRATOR_URL");
            if (string.IsNullOrWhiteSpace(url)) { return null; }

            static string Required(string name) =>
                Environment.GetEnvironmentVariable(name) is { Length: > 0 } v
                    ? v
                    : throw new InvalidOperationException($"MMO zone mode: environment variable {name} is required when MMO_ORCHESTRATOR_URL is set.");
            static string? Optional(string name) =>
                Environment.GetEnvironmentVariable(name) is { Length: > 0 } v ? v : null;

            return new MmoZoneConfig(
                OrchestratorUrl: new Uri(url),
                ZoneSecret: Required("MMO_ZONE_SECRET"),
                ZoneKey: Required("MMO_ZONE_KEY"),
                // The address players should connect to, e.g. "203.0.113.5:27015".
                PublicAddress: Required("MMO_PUBLIC_ADDRESS"),
                // Zone keys reached by leaving through the end / start of this zone's level.
                ExitEndZone: Optional("MMO_EXIT_END"),
                ExitStartZone: Optional("MMO_EXIT_START"),
                // LOAD TESTING ONLY. Unauthenticated IDs are just "UNAUTHENTICATED_" + the player's
                // chosen name, so anyone could take over anyone's character by picking their name.
                AllowUnauthenticated: Optional("MMO_ALLOW_UNAUTHENTICATED") == "I_UNDERSTAND_THIS_IS_INSECURE");
        }
    }

    /// <summary>
    /// Turns a dedicated server into one zone of the MMO world. Hooked into the game at a handful
    /// of points (search for "MmoZone." to find them); everything else lives here so the fork stays
    /// easy to rebase on upstream.
    /// </summary>
    sealed class MmoZone
    {
        public static MmoZone? Instance { get; private set; }

        private const float RegisterRetrySecs = 5f;
        private const float SaveIntervalSecs = 60f;
        private const float RedirectGraceSecs = 15f;
        private const float TransferRetryCooldownSecs = 10f;

        private readonly GameServer server;
        private readonly MmoZoneConfig config;
        private readonly OrchestratorClient api;

        private Guid? instanceId;
        private bool registering;
        private float registerRetryTimer;
        private float heartbeatInterval = 10f;
        private float heartbeatTimer;
        private float saveTimer = SaveIntervalSecs;

        /// <summary>Accounts whose character lease this instance holds.</summary>
        private readonly HashSet<string> leased = new HashSet<string>();
        /// <summary>Accounts we have redirected away, with the time left before we force-disconnect them.</summary>
        private readonly Dictionary<string, float> redirected = new Dictionary<string, float>();

        private bool transferInFlight;
        private float transferCooldown;

        private MmoZone(GameServer server, MmoZoneConfig config)
        {
            this.server = server;
            this.config = config;
            api = new OrchestratorClient(config.OrchestratorUrl, config.ZoneSecret);
        }

        /// <summary>Hook: end of GameServer.StartServer.</summary>
        public static void TryStart(GameServer server)
        {
            MmoZoneConfig? config = MmoZoneConfig.FromEnvironment();
            if (config == null) { return; }
            Instance = new MmoZone(server, config);
            Log($"Zone mode on. Zone \"{config.ZoneKey}\", orchestrator {config.OrchestratorUrl}.");
            if (config.AllowUnauthenticated)
            {
                LogError("MMO_ALLOW_UNAUTHENTICATED is on: players can impersonate each other. Never use this on a public server.");
            }
            Instance.Register();
        }

        private static void Log(string msg) => DebugConsole.NewMessage("[MMO] " + msg, Microsoft.Xna.Framework.Color.Cyan);
        private static void LogError(string msg) => DebugConsole.ThrowError("[MMO] " + msg);

        /// <summary>
        /// The key a character is stored under, or null if this client may not have one. Only
        /// verified accounts (Steam/Epic) qualify, unless load-testing mode is explicitly enabled.
        /// </summary>
        private string? AccountKey(Client c)
        {
            if (!c.AccountId.TryUnwrap(out AccountId? id)) { return null; }
            if (id is UnauthenticatedAccountId && !config.AllowUnauthenticated) { return null; }
            return id.StringRepresentation;
        }

        private static MultiPlayerCampaign? Campaign => GameMain.GameSession?.GameMode as MultiPlayerCampaign;

        /// <summary>Runs an orchestrator call and delivers the outcome on the game thread.</summary>
        private static void Run<T>(string name, Task<T> task, Action<T> onOk, Action<Exception> onError)
        {
            TaskPool.Add(name, task, t =>
            {
                if (t.IsFaulted || t.IsCanceled)
                {
                    onError(t.Exception?.GetBaseException() ?? new TaskCanceledException(name));
                    return;
                }
                onOk(((Task<T>)t).Result);
            });
        }

        private static void Run(string name, Task task, Action onOk, Action<Exception> onError)
        {
            TaskPool.Add(name, task, t =>
            {
                if (t.IsFaulted || t.IsCanceled)
                {
                    onError(t.Exception?.GetBaseException() ?? new TaskCanceledException(name));
                    return;
                }
                onOk();
            });
        }

        // ---------------------------------------------------------------- registration & heartbeat

        private void Register()
        {
            if (registering) { return; }
            registering = true;
            var req = new OrchestratorClient.RegisterRequest(
                config.ZoneKey, config.PublicAddress, server.ServerSettings.ServerName, server.ServerSettings.MaxPlayers);
            Run("MMO.Register", api.Register(req),
                resp =>
                {
                    registering = false;
                    instanceId = resp.InstanceId;
                    heartbeatInterval = Math.Max(1, resp.HeartbeatIntervalSecs);
                    heartbeatTimer = heartbeatInterval;
                    Log($"Registered as instance {resp.InstanceId}.");
                    // Anyone who joined while we were unregistered still needs a claim.
                    foreach (Client c in server.ConnectedClients.ToList())
                    {
                        if (AccountKey(c) is string acc && !leased.Contains(acc) && !redirected.ContainsKey(acc)) { Claim(c); }
                    }
                },
                e =>
                {
                    registering = false;
                    registerRetryTimer = RegisterRetrySecs;
                    LogError($"Could not register with the orchestrator, retrying in {RegisterRetrySecs}s: {e.Message}");
                });
        }

        /// <summary>Hook: GameServer.Update, after the server has started.</summary>
        public void Update(float deltaTime)
        {
            if (instanceId is not Guid id)
            {
                registerRetryTimer -= deltaTime;
                if (registerRetryTimer <= 0f && !registering) { Register(); }
                return;
            }

            heartbeatTimer -= deltaTime;
            if (heartbeatTimer <= 0f)
            {
                heartbeatTimer = heartbeatInterval;
                Run("MMO.Heartbeat", api.Heartbeat(id, server.ConnectedClients.Count),
                    () => { },
                    e =>
                    {
                        if (e is OrchestratorException { Code: "unknown_instance" })
                        {
                            // The orchestrator forgot us (e.g. its database was reset). Start over.
                            LogError("Orchestrator no longer knows this instance; re-registering.");
                            instanceId = null;
                            leased.Clear();
                        }
                        else
                        {
                            LogError($"Heartbeat failed: {e.Message}");
                        }
                    });
            }

            saveTimer -= deltaTime;
            if (saveTimer <= 0f)
            {
                saveTimer = SaveIntervalSecs;
                SaveAll(id);
            }

            if (transferCooldown > 0f) { transferCooldown -= deltaTime; }

            UpdateRedirected(deltaTime);
        }

        private void UpdateRedirected(float deltaTime)
        {
            if (redirected.Count == 0) { return; }
            foreach (string acc in redirected.Keys.ToList())
            {
                float left = redirected[acc] - deltaTime;
                Client? c = server.ConnectedClients.FirstOrDefault(c => AccountKey(c) == acc);
                if (c == null)
                {
                    // They left on their own, which is what we wanted. Keep the entry a little
                    // longer so their disconnect isn't treated as a normal logout.
                    if (left <= -RedirectGraceSecs) { redirected.Remove(acc); } else { redirected[acc] = left; }
                    continue;
                }
                if (left <= 0f)
                {
                    Log($"{c.Name} did not follow the zone redirect; disconnecting.");
                    server.DisconnectClient(c, PeerDisconnectPacket.Custom("You have travelled to another zone. Please reconnect."));
                }
                redirected[acc] = left;
            }
        }

        // ---------------------------------------------------------------- join / leave

        /// <summary>Hook: end of GameServer.OnInitializationComplete (client is Steam-authenticated).</summary>
        public void OnClientJoined(Client client)
        {
            if (AccountKey(client) is not string acc)
            {
                server.DisconnectClient(client, PeerDisconnectPacket.Custom("This MMO server requires a verified Steam or Epic account."));
                return;
            }
            // A new connection from someone we recently sent away (e.g. they travelled back).
            // Forget the redirect so they aren't kicked and their lease is handled normally.
            redirected.Remove(acc);
            if (instanceId == null)
            {
                server.SendDirectChatMessage("Connecting this zone to the world server, one moment...", client);
                return; // claimed once registration completes
            }
            Claim(client);
        }

        private void Claim(Client client)
        {
            if (instanceId is not Guid id || AccountKey(client) is not string acc) { return; }
            Run("MMO.Claim", api.Claim(acc, id),
                resp => OnClaimResult(client, acc, resp),
                e =>
                {
                    LogError($"Claim for {client.Name} ({acc}) failed: {e.Message}");
                    if (server.ConnectedClients.Contains(client))
                    {
                        server.DisconnectClient(client, PeerDisconnectPacket.Custom("The world server is unavailable. Please try again shortly."));
                    }
                });
        }

        private void OnClaimResult(Client client, string acc, OrchestratorClient.ClaimResponse resp)
        {
            bool stillHere = server.ConnectedClients.Contains(client);
            switch (resp.Status)
            {
                case "arrive":
                case "resume":
                    leased.Add(acc);
                    if (!stillHere) { ReleaseLease(acc, xml: null); return; }
                    if (resp.CharacterXml is string xml && Campaign is MultiPlayerCampaign campaign)
                    {
                        try
                        {
                            campaign.MmoSetCharacterData(client, XElement.Parse(xml));
                        }
                        catch (Exception e)
                        {
                            LogError($"Saved character for {client.Name} could not be loaded: {e.Message}");
                        }
                    }
                    else if (Campaign == null)
                    {
                        LogError("Zone servers must be running a multiplayer campaign; character was not loaded.");
                    }
                    Log($"{client.Name} {(resp.Status == "arrive" ? "arrived" : "resumed")}.");
                    break;
                case "new":
                    leased.Add(acc);
                    if (!stillHere) { ReleaseLease(acc, xml: null); return; }
                    Log($"{client.Name} is a new player.");
                    break;
                case "redirect":
                    if (!stillHere) { return; }
                    if (resp.Address is string address)
                    {
                        Redirect(client, acc, resp.ServerName ?? address, address);
                    }
                    else
                    {
                        LogError($"Redirect for {client.Name} had no address.");
                    }
                    break;
                default:
                    LogError($"Unknown claim status \"{resp.Status}\" for {client.Name}.");
                    break;
            }
        }

        /// <summary>
        /// Hook: start of GameServer.DisconnectClient(Client, …), while client.Character still exists.
        /// </summary>
        public void OnClientDisconnecting(Client client)
        {
            if (AccountKey(client) is not string acc) { return; }
            if (redirected.ContainsKey(acc)) { return; } // their lease went with the transfer
            if (!leased.Contains(acc)) { return; }
            ReleaseLease(acc, CaptureCharacterXml(client));
        }

        private void ReleaseLease(string acc, string? xml)
        {
            leased.Remove(acc);
            if (instanceId is not Guid id) { return; }
            Run("MMO.Release", api.Release(acc, id, xml),
                () => { },
                e => LogError($"Release for {acc} failed: {e.Message}"));
        }

        // ---------------------------------------------------------------- saving

        /// <summary>
        /// Serializes the client's current character, or null if there is nothing worth saving
        /// (never spawned, or dead: death rules are an open design decision, so the last save stands).
        /// </summary>
        private static string? CaptureCharacterXml(Client client)
        {
            if (client.CharacterInfo == null) { return null; }
            Character? character = client.Character ?? client.CharacterInfo.Character;
            if (character == null || character.IsDead || character.Removed) { return null; }
            var data = new CharacterCampaignData(client);
            return data.Save().ToString(SaveOptions.DisableFormatting);
        }

        private void SaveAll(Guid id)
        {
            foreach (Client c in server.ConnectedClients)
            {
                if (AccountKey(c) is not string acc || !leased.Contains(acc)) { continue; }
                if (CaptureCharacterXml(c) is not string xml) { continue; }
                Run("MMO.Save", api.Save(acc, id, xml),
                    () => { },
                    e =>
                    {
                        LogError($"Saving {acc} failed: {e.Message}");
                        // Someone else holds this character now; stop writing it.
                        if (e is OrchestratorException { Code: "not_owner" }) { leased.Remove(acc); }
                    });
            }
        }

        // ---------------------------------------------------------------- zone transfers

        /// <summary>
        /// Hook: CampaignMode.LoadNewLevel, after the available transition is known. In zone mode the
        /// level never changes: the leaving crew is sent to the neighbouring zone instead.
        /// Called every frame while a sub sits at an exit, so it debounces itself.
        /// </summary>
        public void OnLevelExit(CampaignMode.TransitionType transition, Submarine? leavingSub)
        {
            if (transferInFlight || transferCooldown > 0f || leavingSub == null) { return; }
            if (instanceId is not Guid id) { return; }

            string? destination = transition switch
            {
                CampaignMode.TransitionType.ReturnToPreviousLocation or
                CampaignMode.TransitionType.ReturnToPreviousEmptyLocation => config.ExitStartZone,
                _ => config.ExitEndZone,
            };

            List<Client> crew = server.ConnectedClients
                .Where(c => c.Character is { IsDead: false, Removed: false } ch &&
                            ch.Submarine != null && leavingSub.IsConnectedTo(ch.Submarine) &&
                            AccountKey(c) is string acc && leased.Contains(acc))
                .ToList();
            if (crew.Count == 0) { return; }

            if (destination == null)
            {
                transferCooldown = TransferRetryCooldownSecs;
                foreach (Client c in crew) { server.SendDirectChatMessage("There is no route out of this zone in that direction.", c); }
                return;
            }

            var players = new List<OrchestratorClient.TransferPlayer>();
            foreach (Client c in crew)
            {
                if (AccountKey(c) is string acc && CaptureCharacterXml(c) is string xml)
                {
                    players.Add(new OrchestratorClient.TransferPlayer(acc, xml));
                }
            }
            if (players.Count == 0) { return; }

            transferInFlight = true;
            Log($"Transferring {players.Count} crew to \"{destination}\".");
            Run("MMO.Transfer", api.Transfer(new OrchestratorClient.TransferRequest(id, destination, players.ToArray())),
                resp =>
                {
                    transferInFlight = false;
                    foreach (Client c in crew)
                    {
                        if (AccountKey(c) is not string acc || !players.Any(p => p.AccountId == acc)) { continue; }
                        leased.Remove(acc);
                        RemoveCharacter(c);
                        Redirect(c, acc, resp.Destination.ServerName, resp.Destination.Address);
                    }
                },
                e =>
                {
                    transferInFlight = false;
                    transferCooldown = TransferRetryCooldownSecs;
                    string reason = e is OrchestratorException { Code: "no_capacity" }
                        ? "The next zone is full or offline. Try again shortly."
                        : $"The route is closed ({e.Message}).";
                    LogError($"Transfer to \"{destination}\" failed: {e.Message}");
                    foreach (Client c in crew)
                    {
                        if (server.ConnectedClients.Contains(c)) { server.SendDirectChatMessage(reason, c); }
                    }
                });
        }

        /// <summary>
        /// Deletes a transferred character and everything it carries from this zone, so nothing is
        /// left behind to duplicate what arrives in the next zone.
        /// </summary>
        private static void RemoveCharacter(Client client)
        {
            // Drop the campaign's copy too, so no vanilla save path can write it back.
            Campaign?.MmoRemoveCharacterData(client);
            Character? character = client.Character;
            if (character == null) { return; }
            character.Inventory?.DeleteAllItems();
            client.Character = null;
            Entity.Spawner?.AddEntityToRemoveQueue(character);
        }

        private void Redirect(Client client, string acc, string serverName, string address)
        {
            redirected[acc] = RedirectGraceSecs;
            server.SendZoneRedirect(client, serverName, address);
            Log($"Redirected {client.Name} to {serverName} ({address}).");
        }
    }
}
