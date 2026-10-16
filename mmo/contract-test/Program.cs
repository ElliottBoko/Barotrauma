// Contract test: the game's OrchestratorClient against the real Rust orchestrator.
using System;
using System.Threading.Tasks;
using Barotrauma.Networking.Mmo;

static class Program
{
    static int failures;
    static void Check(bool ok, string what) { Console.WriteLine((ok ? "PASS " : "FAIL ") + what); if (!ok) failures++; }

    static async Task<int> Main(string[] args)
    {
        var api = new OrchestratorClient(new Uri(args[0]), args[1]);
        const string P = "STEAM_1:1:777";
        const string Xml = "<CharacterCampaignData name=\"Ada\" accountid=\"STEAM_1:1:777\"><inventory><Item name=\"divingsuit\" /></inventory></CharacterCampaignData>";

        var a = await api.Register(new OrchestratorClient.RegisterRequest("loc-0", "127.0.0.1:27015", "Surface", 32));
        var b = await api.Register(new OrchestratorClient.RegisterRequest("conn-0-1", "127.0.0.1:27016", "Caverns", 32));
        Check(a.InstanceId != Guid.Empty && a.HeartbeatIntervalSecs == 10, "register returns instance id and heartbeat interval");
        await api.Heartbeat(a.InstanceId, 1);
        Check(true, "heartbeat");

        var c1 = await api.Claim(P, a.InstanceId);
        Check(c1.Status == "new", $"first join at entry zone is new ({c1.Status})");
        await api.Save(P, a.InstanceId, Xml);
        Check(true, "save");

        var redirect = await api.Claim(P, b.InstanceId);
        Check(redirect.Status == "redirect" && redirect.Address == "127.0.0.1:27015" && redirect.ServerName == "Surface"
              && redirect.InstanceId == a.InstanceId, "joining wrong zone redirects with address, name and instance id");

        var t = await api.Transfer(new OrchestratorClient.TransferRequest(a.InstanceId, "conn-0-1",
            new[] { new OrchestratorClient.TransferPlayer(P, Xml) }));
        Check(t.Destination.InstanceId == b.InstanceId && t.Destination.Address == "127.0.0.1:27016" && t.ExpiresInSecs == 300,
              "transfer returns destination");

        try { await api.Save(P, a.InstanceId, Xml); Check(false, "old zone can no longer save"); }
        catch (OrchestratorException e) { Check(e.Code == "not_owner" && (int)e.Status == 409, $"old zone gets not_owner ({e.Code})"); }

        var arrive = await api.Claim(P, b.InstanceId);
        Check(arrive.Status == "arrive" && arrive.CharacterXml == Xml, "arrival returns the exact character XML");

        await api.Release(P, b.InstanceId, null);
        var resume = await api.Claim(P, b.InstanceId);
        Check(resume.Status == "resume" && resume.CharacterXml == Xml, "release with null XML keeps the last save");

        try { await api.Transfer(new OrchestratorClient.TransferRequest(b.InstanceId, "loc-2", new[] { new OrchestratorClient.TransferPlayer(P, Xml) })); Check(false, "non-adjacent transfer refused"); }
        catch (OrchestratorException e) { Check(e.Code == "not_adjacent", $"non-adjacent transfer refused ({e.Code})"); }

        try { await api.Heartbeat(Guid.NewGuid(), 0); Check(false, "unknown instance heartbeat refused"); }
        catch (OrchestratorException e) { Check(e.Code == "unknown_instance", $"unknown instance detected ({e.Code})"); }

        var bad = new OrchestratorClient(new Uri(args[0]), "wrong-secret-xxxxxxxx");
        try { await bad.Heartbeat(a.InstanceId, 0); Check(false, "wrong secret refused"); }
        catch (OrchestratorException e) { Check(e.Code == "unauthorized", $"wrong secret refused ({e.Code})"); }

        Console.WriteLine(failures == 0 ? "ALL PASSED" : $"{failures} FAILED");
        return failures == 0 ? 0 : 1;
    }
}
