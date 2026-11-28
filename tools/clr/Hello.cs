// A small 32-bit program (testdata/clr/Hello.exe): an entry point, a PE32
// image, 32BITREQUIRED.

static class Program
{
    static int Main(string[] args)
    {
        System.Console.WriteLine("Hello");
        return args.Length;
    }
}
