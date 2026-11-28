// Writes the sample's .resources (System.Resources.ResourceWriter's
// format): one of each type it stores natively.

using System;
using System.IO;
using System.Resources;

static class ResGen
{
    static void Main(string[] args)
    {
        using var w = new ResourceWriter(args[0]);
        w.AddResource("greeting", "Hello, resources");
        w.AddResource("empty", "");
        w.AddResource("nothing", (object)null);
        w.AddResource("flag", true);
        w.AddResource("letter", 'q');
        w.AddResource("byte", (byte)200);
        w.AddResource("sbyte", (sbyte)-100);
        w.AddResource("short", (short)-1234);
        w.AddResource("ushort", (ushort)65000);
        w.AddResource("int", -123456);
        w.AddResource("uint", 4000000000u);
        w.AddResource("long", -1234567890123L);
        w.AddResource("ulong", 18000000000000000000UL);
        w.AddResource("float", 1.25f);
        w.AddResource("double", -2.5);
        w.AddResource("decimal", 12345.6789m);
        w.AddResource("date", new DateTime(2020, 2, 29, 12, 34, 56, DateTimeKind.Utc));
        w.AddResource("span", new TimeSpan(1, 2, 3, 4, 5));
        w.AddResource("bytes", new byte[] { 1, 2, 3, 4, 5 });
        w.AddResource("stream", new MemoryStream(new byte[] { 9, 8, 7 }));
        w.AddResource("unicode \u00e9", "\u2603 snow");
        w.Generate();
    }
}
