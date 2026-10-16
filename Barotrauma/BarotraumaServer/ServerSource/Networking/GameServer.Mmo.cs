// Barotrauma MMO mod. This file is part of a modified version of Barotrauma;
// see docs/mmo/DESIGN.md. Not made or endorsed by Undertow Games.

namespace Barotrauma.Networking
{
    partial class GameServer
    {
        /// <summary>
        /// Tells a client to leave and connect to another zone server. The client handles this by
        /// queueing a connect command, which quits this server and joins the other one.
        /// </summary>
        public void SendZoneRedirect(Client client, string serverName, string endpoint)
        {
            if (client?.Connection == null || serverPeer == null) { return; }
            IWriteMessage msg = new WriteOnlyMessage();
            msg.WriteByte((byte)ServerPacketHeader.ZONE_REDIRECT);
            msg.WriteString(serverName ?? string.Empty);
            msg.WriteString(endpoint ?? string.Empty);
            serverPeer.Send(msg, client.Connection, DeliveryMethod.Reliable);
        }
    }
}
