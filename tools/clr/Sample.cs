// The sample assembly (testdata/clr/Sample.dll): a little of everything
// ECMA-335 metadata can hold, for turbonop-clr's tests. See build.cmd.
// (ASCII only: other characters are escapes.)

using System;
using System.Collections.Generic;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Security;
using System.Security.Permissions;

[assembly: System.Reflection.AssemblyVersion("1.2.3.4")]
[assembly: System.Reflection.AssemblyTitle("Sample \u00e9")]
[assembly: System.Reflection.AssemblyFileVersion("5.6.7.8")]
[assembly: TypeForwardedTo(typeof(System.Collections.Generic.Stack<>))]
[assembly: Sample.Note("assembly", Sample.Color.Red, typeof(int), Sample.Access.All, null)]
[module: DefaultCharSet(CharSet.Unicode)]

#pragma warning disable CS0067, CS0169, CS0414, CS0649, CS0618, SYSLIB0003

namespace Sample
{
    public enum Color : byte { Red, Green = 5, Blue }

    [Flags]
    public enum Access : long { None = 0, Read = 1, Write = 2, All = Read | Write }

    [AttributeUsage(AttributeTargets.All, AllowMultiple = true)]
    public sealed class NoteAttribute : Attribute
    {
        public NoteAttribute(string text, Color color, Type type, object boxed, int[] values) { }
        public NoteAttribute(char c, sbyte b, ulong u, float f, double d, bool flag) { }
        public NoteAttribute(object[] many) { }
        public int Number;
        public Type[] Types;
        public string Named { get; set; }
    }

    // DeclSecurity: an attribute deriving from SecurityAttribute.
    [AttributeUsage(AttributeTargets.All, AllowMultiple = true)]
    public sealed class GuardAttribute : CodeAccessSecurityAttribute
    {
        public GuardAttribute(SecurityAction action) : base(action) { }
        public string Level { get; set; }
        public override IPermission CreatePermission() => null;
    }

    public interface IShape
    {
        double Area { get; }
        event EventHandler Changed;
        static abstract IShape Unit();
        string Describe() => "shape";
    }

    [StructLayout(LayoutKind.Explicit, Size = 16, Pack = 4)]
    public struct Union
    {
        [FieldOffset(0)] public int I;
        [FieldOffset(0)] public float F;
        [FieldOffset(8)] public long L;
    }

    [StructLayout(LayoutKind.Sequential, Pack = 1)]
    public unsafe struct Native
    {
        [MarshalAs(UnmanagedType.LPWStr)] public string Name;
        [MarshalAs(UnmanagedType.ByValArray, SizeConst = 4, ArraySubType = UnmanagedType.I4)] public int[] Four;
        [MarshalAs(UnmanagedType.LPArray, SizeParamIndex = 1)] public int[] Counted;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string Fixed;
        [MarshalAs(UnmanagedType.Interface)] public object Unknown;
        public fixed byte Buffer[8];
        public delegate* unmanaged[Cdecl]<int, int> Callback;
        public delegate* managed<ref int, string> Managed;
    }

    [Note("type", Color.Blue, typeof(List<int>), 42, new[] { 1, 2, 3 }, Number = 7, Named = "n")]
    [Note('c', -5, 0xFFFFFFFFFFFFFFFF, 1.5f, -2.25, true, Types = new[] { typeof(string), typeof(Circle.Nested<int>) })]
    [Note(new object[] { 1, "two", Color.Green, typeof(Union), null, new[] { 3L } })]
    [Guard(SecurityAction.Demand, Level = "high")]
    [Serializable]
    public class Circle : IShape, IComparable<Circle>
    {
        public const double Pi = 3.14159;
        public const string Label = "circle\u00e9\u2603";
        public const char Letter = 'x';
        public const Color Favorite = Color.Blue;
        public const object Nothing = null;
        public static readonly byte[] Table = { 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17 };
        public static ReadOnlySpan<int> Primes => new[] { 2, 3, 5, 7, 11, 13 };
        private double radius;
        public volatile int Version;
        [NonSerialized] public int Transient;
        public event EventHandler Changed;
        public double Area => Pi * radius * radius;
        public double this[int i] { get => i * radius; set => radius = value / i; }
        public static IShape Unit() => new Circle(1);
        public Circle(double r) { radius = r; }
        double IShape.Area => Area;
        public int CompareTo(Circle other) => radius.CompareTo(other?.radius ?? 0);
        protected virtual void OnChanged() => Changed?.Invoke(this, EventArgs.Empty);

        public class Nested<T> where T : struct, IComparable<T>
        {
            public T Value;
            public class Deeper<U> where U : class, new() { public U Inner; public T Outer; }
        }
    }

    public static class Code
    {
        [DllImport("kernel32.dll", EntryPoint = "GetTickCount", SetLastError = true, ExactSpelling = true)]
        public static extern uint Ticks();

        [DllImport("user32", CharSet = CharSet.Unicode, CallingConvention = CallingConvention.Winapi)]
        public static extern int MessageBoxW(IntPtr h, [MarshalAs(UnmanagedType.LPWStr)] string text, string caption, uint type);

        [DllImport("libc", EntryPoint = "abs", CallingConvention = CallingConvention.Cdecl)]
        [return: MarshalAs(UnmanagedType.I4)]
        public static extern int Abs([MarshalAs(UnmanagedType.I4)] int x);

        public static int Switch(int x)
        {
            switch (x)
            {
                case 0: return 10;
                case 1: return 20;
                case 2: return 30;
                case 3: return 45;
                case 4: return 99;
                default: return -1;
            }
        }

        public static string Exceptions(int x)
        {
            try
            {
                try
                {
                    if (x > 0) throw new InvalidOperationException("x");
                }
                catch (InvalidOperationException e) when (e.Message == "x")
                {
                    return "filtered";
                }
                catch (Exception)
                {
                    return "caught";
                }
                finally
                {
                    Console.WriteLine("finally");
                }
            }
            catch
            {
            }
            return "none";
        }

        public static unsafe int Pointers(int[] a)
        {
            fixed (int* p = a)
            {
                return *p + p[1];
            }
        }

        public static ref int RefReturn(int[] a) => ref a[0];
        public static int Modes(in int x, out int y, ref int z) { y = x; return z; }
        public static int VarArgs(int first, __arglist) { var it = new ArgIterator(__arglist); return first + it.GetRemainingCount(); }
        public static int CallVarArgs() => VarArgs(0, __arglist(1, "two", 3.0));
        public static int TypedRef() { int i = 5; TypedReference tr = __makeref(i); return __refvalue(tr, int); }
        public static T Generic<T, U>(T t, U u) where T : class, new() where U : IEnumerable<T> => t ?? new T();
        public static object UseGeneric() => Generic<object, List<object>>(null, null);
        public static int[,] Matrix() => new int[2, 3];
        public static int[][] Jagged() => new int[2][];
        public static long Defaults(long a = 5, string s = "d", Color c = Color.Green, double d = 1.5, object o = null) => a;
        public static string Strings() => "hello\tworld\u2603\0end \"quoted\" \\ \r\n" + "\u00ff";
        public static float Floats(float f) => f * 2.5f + (float)Math.Sqrt(9.0);
        public static long Big() => 0x123456789ABCDEF;
        public static IEnumerable<int> Iterator() { yield return 1; yield return 2; }
        public static async System.Threading.Tasks.Task<int> Async() { await System.Threading.Tasks.Task.Yield(); return 1; }
        public static Func<int, int> Lambda(int k) => x => x + k;
        public static unsafe int StackAlloc() { int* p = stackalloc int[4]; p[0] = 1; return sizeof(Union) + p[0]; }
        public static int Checked(int a, uint b) => checked(a * (int)b);
        public static unsafe int CallFnPtr(delegate*<int, int> f) => f(3);
        public static unsafe delegate*<int, int> FnPtr() => &Square;
        static int Square(int x) => x * x;
        public static nint NativeInts(nint a, nuint b) => a + (nint)b;
        public static (int, string) Tuple() => (1, "a");
        public static int Locals(int n)
        {
            int a = n, b = 2, c = 3, d = 4, e = 5;
            string s = n.ToString();
            for (int i = 0; i < n; i++) { a += b * c - d / e + s.Length; }
            return a;
        }
        public static void Volatile(ref int x) { System.Threading.Volatile.Write(ref x, 1); }
        public static int Unbox(object o) => (int)o + (o is string ? 1 : 0);
        public static T Constrained<T>(T t) where T : IComparable<T> => t.CompareTo(t) > 0 ? t : default;
        public static Type Token() => typeof(Circle);
        public static RuntimeMethodHandle MethodToken() => default;
        public static unsafe void Blocks(byte* a, byte* b) { Unsafe.CopyBlock(a, b, 8); Unsafe.InitBlock(a, 0, 8); }
        public static Circle.Nested<int>.Deeper<object> DeepGeneric() => null;
    }

    public abstract class Base<T>
    {
        public abstract T Get();
        public virtual void Set(T t) { }
        public static int Count;
    }

    public sealed class Derived : Base<string>
    {
        public override string Get() => "d";
        public override void Set(string s) => base.Set(s);
        public int UseCount() => Count;
    }

    public delegate int Transform(int x);
    public record Point(int X, int Y);

    public struct Holder<T> where T : unmanaged
    {
        public T Item;
        public static Holder<T> Make(T t) => new Holder<T> { Item = t };
    }
}
