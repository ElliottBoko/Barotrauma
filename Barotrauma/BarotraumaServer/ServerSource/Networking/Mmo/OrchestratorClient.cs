// Barotrauma MMO mod. This file is part of a modified version of Barotrauma;
// see docs/mmo/DESIGN.md. Not made or endorsed by Undertow Games.
#nullable enable

using System;
using System.Net;
using System.Net.Http;
using System.Net.Http.Headers;
using System.Net.Http.Json;
using System.Text.Json;
using System.Threading.Tasks;

namespace Barotrauma.Networking.Mmo
{
    /// <summary>
    /// HTTP client for the orchestrator (see docs/mmo/PROTOCOL.md). All calls are async and
    /// must be awaited off the game thread; <see cref="MmoZone"/> marshals results back through
    /// <see cref="TaskPool"/>.
    /// </summary>
    sealed class OrchestratorClient : IDisposable
    {
        public static readonly JsonSerializerOptions Json = new JsonSerializerOptions
        {
            PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower,
            DefaultIgnoreCondition = System.Text.Json.Serialization.JsonIgnoreCondition.WhenWritingNull,
        };

        private readonly HttpClient http;

        public OrchestratorClient(Uri baseUrl, string zoneSecret)
        {
            // A trailing slash makes relative paths append to the base instead of replacing its last segment.
            string url = baseUrl.ToString();
            if (!url.EndsWith('/')) { url += "/"; }
            http = new HttpClient
            {
                BaseAddress = new Uri(url),
                Timeout = TimeSpan.FromSeconds(10),
            };
            http.DefaultRequestHeaders.Authorization = new AuthenticationHeaderValue("Bearer", zoneSecret);
        }

        public record RegisterRequest(string ZoneKey, string Address, string ServerName, int MaxPlayers);
        public record RegisterResponse(Guid InstanceId, int HeartbeatIntervalSecs);
        public record HeartbeatRequest(int PlayerCount);
        public record InstanceRequest(Guid InstanceId);
        public record SaveRequest(Guid InstanceId, string? CharacterXml);
        public record TransferPlayer(string AccountId, string CharacterXml);
        public record TransferRequest(Guid InstanceId, string DestinationZoneKey, TransferPlayer[] Players);
        public record Destination(Guid InstanceId, string Address, string ServerName);
        public record TransferResponse(Guid TransferId, Destination Destination, int ExpiresInSecs);

        /// <summary>
        /// status is one of "arrive", "resume", "new", "redirect".
        /// arrive/resume carry CharacterXml; redirect carries InstanceId, Address and ServerName.
        /// </summary>
        public record ClaimResponse(string Status, string? CharacterXml, Guid? InstanceId, string? Address, string? ServerName);

        private record ErrorBody(string? Error, string? Message);

        public Task<RegisterResponse> Register(RegisterRequest req)
            => Send<RegisterResponse>(HttpMethod.Post, "v1/zones/register", req);

        public Task Heartbeat(Guid instanceId, int playerCount)
            => Send(HttpMethod.Post, $"v1/zones/{instanceId}/heartbeat", new HeartbeatRequest(playerCount));

        public Task<ClaimResponse> Claim(string accountId, Guid instanceId)
            => Send<ClaimResponse>(HttpMethod.Post, $"v1/players/{Uri.EscapeDataString(accountId)}/claim", new InstanceRequest(instanceId));

        public Task Save(string accountId, Guid instanceId, string characterXml)
            => Send(HttpMethod.Put, $"v1/players/{Uri.EscapeDataString(accountId)}/character", new SaveRequest(instanceId, characterXml));

        /// <param name="characterXml">Null keeps the last saved character (e.g. the player never spawned).</param>
        public Task Release(string accountId, Guid instanceId, string? characterXml)
            => Send(HttpMethod.Post, $"v1/players/{Uri.EscapeDataString(accountId)}/release", new SaveRequest(instanceId, characterXml));

        public Task<TransferResponse> Transfer(TransferRequest req)
            => Send<TransferResponse>(HttpMethod.Post, "v1/transfers", req);

        private async Task<T> Send<T>(HttpMethod method, string path, object body)
        {
            using HttpResponseMessage resp = await SendRaw(method, path, body).ConfigureAwait(false);
            T? result = await resp.Content.ReadFromJsonAsync<T>(Json).ConfigureAwait(false);
            return result ?? throw new OrchestratorException(resp.StatusCode, "bad_response", $"Empty response from {path}");
        }

        private async Task Send(HttpMethod method, string path, object body)
        {
            using HttpResponseMessage resp = await SendRaw(method, path, body).ConfigureAwait(false);
        }

        private async Task<HttpResponseMessage> SendRaw(HttpMethod method, string path, object body)
        {
            using var req = new HttpRequestMessage(method, path)
            {
                Content = JsonContent.Create(body, body.GetType(), options: Json),
            };
            HttpResponseMessage resp = await http.SendAsync(req).ConfigureAwait(false);
            if (resp.IsSuccessStatusCode) { return resp; }

            ErrorBody? err = null;
            try { err = await resp.Content.ReadFromJsonAsync<ErrorBody>(Json).ConfigureAwait(false); }
            catch (Exception) { /* not JSON; fall through with a generic error */ }
            HttpStatusCode status = resp.StatusCode;
            resp.Dispose();
            throw new OrchestratorException(status, err?.Error ?? "http_error", err?.Message ?? $"{(int)status} {status} from {path}");
        }

        public void Dispose() => http.Dispose();
    }

    sealed class OrchestratorException : Exception
    {
        public readonly HttpStatusCode Status;
        /// <summary>Machine-readable code from the orchestrator, e.g. "not_owner", "no_capacity".</summary>
        public readonly string Code;

        public OrchestratorException(HttpStatusCode status, string code, string message) : base(message)
        {
            Status = status;
            Code = code;
        }
    }
}
