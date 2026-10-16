// Barotrauma MMO mod. This file is part of a modified version of Barotrauma;
// see docs/mmo/DESIGN.md. Not made or endorsed by Undertow Games.
#nullable enable

using System.Xml.Linq;
using Barotrauma.Networking;

namespace Barotrauma
{
    partial class MultiPlayerCampaign
    {
        /// <summary>
        /// Installs a character that arrived from another zone (or was resumed from the database) as
        /// this client's campaign character. It has not spawned in this zone yet, so the respawn
        /// manager spawns it mid-round with its saved inventory, health and wallet.
        /// </summary>
        public void MmoSetCharacterData(Client client, XElement characterCampaignData)
        {
            characterData.RemoveAll(cd => cd.MatchesClient(client));
            var data = new CharacterCampaignData(characterCampaignData) { HasSpawned = false };
            characterData.Add(data);
            client.CharacterInfo = data.CharacterInfo;
            IncrementLastUpdateIdForFlag(NetFlags.CharacterInfo);
        }

        /// <summary>Forgets a client's character after it has left this zone.</summary>
        public void MmoRemoveCharacterData(Client client)
        {
            if (characterData.RemoveAll(cd => cd.MatchesClient(client)) > 0)
            {
                IncrementLastUpdateIdForFlag(NetFlags.CharacterInfo);
            }
        }
    }
}
