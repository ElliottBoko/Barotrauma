// Barotrauma MMO mod. This file is part of a modified version of Barotrauma;
// see docs/mmo/DESIGN.md. Not made or endorsed by Undertow Games.

using Microsoft.Xna.Framework;

namespace Barotrauma.Networking
{
    partial class GameClient
    {
        /// <summary>
        /// The server is sending us to another zone. Queue a connect command; GameMain's update loop
        /// quits this connection and joins the new server on the next frame.
        /// </summary>
        private void ReadZoneRedirect(IReadMessage inc)
        {
            string serverName = inc.ReadString();
            string endpointStr = inc.ReadString();

            if (!Endpoint.Parse(endpointStr).TryUnwrap(out Endpoint endpoint))
            {
                DebugConsole.ThrowError($"[MMO] Received a zone redirect with an invalid endpoint: \"{endpointStr}\"");
                return;
            }

            DebugConsole.NewMessage($"[MMO] Travelling to {serverName} ({endpointStr})...", Color.Cyan);
            GameMain.Instance.ConnectCommand = Option.Some(new ConnectCommand(serverName, endpoint));
        }
    }
}
