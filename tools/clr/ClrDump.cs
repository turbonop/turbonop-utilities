// turbonop-clr's oracle: an assembly's (or portable PDB's) metadata as
// System.Reflection.Metadata reads it, in a text form that
// crates/formats/clr/tests/oracle.rs renders the same way from
// turbonop-clr. Every row of every table, by SRM's own accessors; method
// bodies with their IL decoded by System.Reflection.Emit's opcode table;
// signatures and custom attributes decoded by SRM's decoders, rendered by
// the providers below; .resources entries by System.Resources.ResourceReader.
//
//   ClrDump <assembly.dll | file.pdb>     the dump
//   ClrDump -opcodes                      the CIL opcode table
//
// See build.cmd (it builds this against the .NET 8 reference assemblies).

using System;
using System.Collections.Generic;
using System.Collections.Immutable;
using System.IO;
using System.Linq;
using System.Reflection;
using System.Reflection.Emit;
using System.Reflection.Metadata;
using System.Reflection.Metadata.Ecma335;
using System.Reflection.PortableExecutable;
using System.Text;

static class ClrDump
{
    static TextWriter Out;

    static int Main(string[] args)
    {
        var stdout = new StreamWriter(Console.OpenStandardOutput(), new UTF8Encoding(false));
        stdout.NewLine = "\n";
        Out = stdout;
        try
        {
            if (args.Length == 1 && args[0] == "-opcodes")
            {
                Opcodes();
            }
            else if (args.Length == 1)
            {
                Dump(args[0]);
            }
            else
            {
                Console.Error.WriteLine("usage: ClrDump <file> | -opcodes");
                return 2;
            }
        }
        finally
        {
            stdout.Flush();
        }
        return 0;
    }

    static readonly Dictionary<ushort, OpCode> OpcodeTable = BuildOpcodes();

    static Dictionary<ushort, OpCode> BuildOpcodes()
    {
        var table = new Dictionary<ushort, OpCode>();
        foreach (var f in typeof(OpCodes).GetFields(BindingFlags.Public | BindingFlags.Static))
        {
            var op = (OpCode)f.GetValue(null);
            if (op.OpCodeType == OpCodeType.Nternal)
            {
                continue;
            }
            table[(ushort)op.Value] = op;
        }
        return table;
    }

    static void Opcodes()
    {
        foreach (var kv in OpcodeTable.OrderBy(kv => kv.Key))
        {
            Out.WriteLine($"0x{kv.Key:X4} {kv.Value.Name} {kv.Value.OperandType}");
        }
    }

    static void Dump(string path)
    {
        var bytes = File.ReadAllBytes(path);
        if (bytes.Length >= 4 && bytes[0] == (byte)'B' && bytes[1] == (byte)'S' && bytes[2] == (byte)'J' && bytes[3] == (byte)'B')
        {
            var provider = MetadataReaderProvider.FromPortablePdbImage(ImmutableArray.Create(bytes));
            DumpMetadata(provider.GetMetadataReader(), null);
            return;
        }
        using var pe = new PEReader(ImmutableArray.Create(bytes));
        var cor = pe.PEHeaders.CorHeader;
        Out.WriteLine($"cor20 runtime={cor.MajorRuntimeVersion}.{cor.MinorRuntimeVersion} flags=0x{(uint)cor.Flags:x8} entrypoint=0x{cor.EntryPointTokenOrRelativeVirtualAddress:x8}");
        Out.WriteLine($"cor20 metadata={Dir(cor.MetadataDirectory)} resources={Dir(cor.ResourcesDirectory)} strongname={Dir(cor.StrongNameSignatureDirectory)}");
        Out.WriteLine($"cor20 codemanager={Dir(cor.CodeManagerTableDirectory)} vtablefixups={Dir(cor.VtableFixupsDirectory)} eatjumps={Dir(cor.ExportAddressTableJumpsDirectory)} managednative={Dir(cor.ManagedNativeHeaderDirectory)}");
        DumpMetadata(pe.GetMetadataReader(), pe);
    }

    static string Dir(DirectoryEntry d) => $"0x{d.RelativeVirtualAddress:x}+0x{d.Size:x}";

    static void DumpMetadata(MetadataReader md, PEReader pe)
    {
        var p = new Provider(md);
        Out.WriteLine($"metadata version={md.MetadataVersion}");
        foreach (HeapIndex h in new[] { HeapIndex.String, HeapIndex.UserString, HeapIndex.Guid, HeapIndex.Blob })
        {
            // (An absent heap's offset is meaningless.)
            int size = md.GetHeapSize(h);
            Out.WriteLine($"heap {h} offset=0x{(size == 0 ? 0 : md.GetHeapMetadataOffset(h)):x} size=0x{size:x}");
        }
        foreach (TableIndex t in Enum.GetValues<TableIndex>().Distinct().OrderBy(t => (int)t))
        {
            int rows = md.GetTableRowCount(t);
            if (rows != 0)
            {
                Out.WriteLine($"table {t} rows={rows} rowsize={md.GetTableRowSize(t)} offset=0x{md.GetTableMetadataOffset(t):x}");
            }
        }
        if (md.DebugMetadataHeader is { } dh)
        {
            Out.WriteLine($"pdb id={Hex(dh.Id)} entrypoint={Tok(dh.EntryPoint)}");
        }

        if (md.GetTableRowCount(TableIndex.Module) > 0)
        {
            var m = md.GetModuleDefinition();
            Out.WriteLine($"Module[1] generation={m.Generation} name={md.GetString(m.Name)} mvid={G(md, m.Mvid)} encid={G(md, m.GenerationId)} encbaseid={G(md, m.BaseGenerationId)}");
        }
        foreach (var h in md.TypeReferences)
        {
            var t = md.GetTypeReference(h);
            Out.WriteLine($"TypeRef[{Rid(h)}] scope={Tok(t.ResolutionScope)} ns={md.GetString(t.Namespace)} name={md.GetString(t.Name)} full={p.Name(h)}");
        }
        foreach (var h in md.TypeDefinitions)
        {
            var t = md.GetTypeDefinition(h);
            Out.WriteLine($"TypeDef[{Rid(h)}] flags=0x{(uint)t.Attributes:x8} ns={md.GetString(t.Namespace)} name={md.GetString(t.Name)} full={p.Name(h)} extends={TypeOrNil(p, t.BaseType)}");
            var enclosing = t.GetDeclaringType();
            if (!enclosing.IsNil)
            {
                Out.WriteLine($"  enclosing {Tok(enclosing)}");
            }
            var layout = t.GetLayout();
            if (!layout.IsDefault)
            {
                Out.WriteLine($"  layout pack={layout.PackingSize} size={layout.Size}");
            }
            foreach (var i in t.GetInterfaceImplementations())
            {
                var ii = md.GetInterfaceImplementation(i);
                Out.WriteLine($"  InterfaceImpl[{Rid(i)}] {p.TypeToken(ii.Interface)}");
            }
            GenericParams(md, p, t.GetGenericParameters());
            Out.WriteLine($"  fields {Ranges(t.GetFields().Select(x => Rid(x)))}");
            Out.WriteLine($"  methods {Ranges(t.GetMethods().Select(x => Rid(x)))}");
            Out.WriteLine($"  properties {Ranges(t.GetProperties().Select(x => Rid(x)))}");
            Out.WriteLine($"  events {Ranges(t.GetEvents().Select(x => Rid(x)))}");
            Out.WriteLine($"  nested {Ranges(t.GetNestedTypes().Select(x => Rid(x)))}");
        }
        for (int rid = 1; rid <= md.GetTableRowCount(TableIndex.Field); rid++)
        {
            var h = MetadataTokens.FieldDefinitionHandle(rid);
            var f = md.GetFieldDefinition(h);
            Out.WriteLine($"Field[{rid}] flags=0x{(ushort)f.Attributes:x4} name={md.GetString(f.Name)} sig={Try(() => f.DecodeSignature(p, null))} owner={Tok(f.GetDeclaringType())}");
            Constant(md, f.GetDefaultValue());
            if (f.GetRelativeVirtualAddress() != 0)
            {
                Out.WriteLine($"  rva 0x{f.GetRelativeVirtualAddress():x}");
            }
            if (f.GetOffset() != -1)
            {
                Out.WriteLine($"  offset {f.GetOffset()}");
            }
            if (!f.GetMarshallingDescriptor().IsNil)
            {
                Out.WriteLine($"  marshal {Hex(md.GetBlobBytes(f.GetMarshallingDescriptor()))}");
            }
        }
        for (int rid = 1; rid <= md.GetTableRowCount(TableIndex.MethodDef); rid++)
        {
            var h = MetadataTokens.MethodDefinitionHandle(rid);
            var m = md.GetMethodDefinition(h);
            Out.WriteLine($"MethodDef[{rid}] rva=0x{m.RelativeVirtualAddress:x} impl=0x{(ushort)m.ImplAttributes:x4} flags=0x{(ushort)m.Attributes:x4} name={md.GetString(m.Name)} sig={Try(() => p.MethodSig(m.DecodeSignature(p, null)))} owner={Tok(m.GetDeclaringType())}");
            Out.WriteLine($"  params {Ranges(m.GetParameters().Select(x => Rid(x)))}");
            GenericParams(md, p, m.GetGenericParameters());
            var import = m.GetImport();
            if (!import.Module.IsNil)
            {
                Out.WriteLine($"  import flags=0x{(ushort)import.Attributes:x4} name={md.GetString(import.Name)} module={Tok(import.Module)}");
            }
            if (m.RelativeVirtualAddress != 0 && pe != null)
            {
                Body(md, p, pe, m.RelativeVirtualAddress);
            }
        }
        for (int rid = 1; rid <= md.GetTableRowCount(TableIndex.Param); rid++)
        {
            var h = MetadataTokens.ParameterHandle(rid);
            var a = md.GetParameter(h);
            Out.WriteLine($"Param[{rid}] flags=0x{(ushort)a.Attributes:x4} seq={a.SequenceNumber} name={md.GetString(a.Name)}");
            Constant(md, a.GetDefaultValue());
            if (!a.GetMarshallingDescriptor().IsNil)
            {
                Out.WriteLine($"  marshal {Hex(md.GetBlobBytes(a.GetMarshallingDescriptor()))}");
            }
        }
        for (int rid = 1; rid <= md.GetTableRowCount(TableIndex.MemberRef); rid++)
        {
            var h = MetadataTokens.MemberReferenceHandle(rid);
            var r = md.GetMemberReference(h);
            Out.WriteLine($"MemberRef[{rid}] parent={Tok(r.Parent)} name={md.GetString(r.Name)} full={Try(() => p.Member(h))}");
        }
        for (int rid = 1; rid <= md.GetTableRowCount(TableIndex.Constant); rid++)
        {
            var c = md.GetConstant(MetadataTokens.ConstantHandle(rid));
            Out.WriteLine($"Constant[{rid}] type=0x{(byte)c.TypeCode:x2} parent={Tok(c.Parent)} value={Hex(md.GetBlobBytes(c.Value))}");
        }
        for (int rid = 1; rid <= md.GetTableRowCount(TableIndex.CustomAttribute); rid++)
        {
            var c = md.GetCustomAttribute(MetadataTokens.CustomAttributeHandle(rid));
            Out.WriteLine($"CustomAttribute[{rid}] parent={Tok(c.Parent)} ctor={Try(() => p.Member(c.Constructor))} value={Attribute(md, p, c)}");
        }
        for (int rid = 1; rid <= md.GetTableRowCount(TableIndex.DeclSecurity); rid++)
        {
            var d = md.GetDeclarativeSecurityAttribute(MetadataTokens.DeclarativeSecurityAttributeHandle(rid));
            Out.WriteLine($"DeclSecurity[{rid}] action={(ushort)d.Action} parent={Tok(d.Parent)} set={Hex(md.GetBlobBytes(d.PermissionSet))}");
        }
        for (int rid = 1; rid <= md.GetTableRowCount(TableIndex.StandAloneSig); rid++)
        {
            var s = md.GetStandaloneSignature(MetadataTokens.StandaloneSignatureHandle(rid));
            string sig = Try(() => s.GetKind() == StandaloneSignatureKind.LocalVariables
                ? "locals (" + string.Join(", ", s.DecodeLocalSignature(p, null)) + ")"
                : p.MethodSig(s.DecodeMethodSignature(p, null)));
            Out.WriteLine($"StandAloneSig[{rid}] {sig}");
        }
        for (int rid = 1; rid <= md.GetTableRowCount(TableIndex.Event); rid++)
        {
            var h = MetadataTokens.EventDefinitionHandle(rid);
            var e = md.GetEventDefinition(h);
            var a = e.GetAccessors();
            Out.WriteLine($"Event[{rid}] flags=0x{(ushort)e.Attributes:x4} name={md.GetString(e.Name)} type={p.TypeToken(e.Type)} add={Tok(a.Adder)} remove={Tok(a.Remover)} raise={Tok(a.Raiser)} others={string.Join(",", a.Others.Select(x => Rid(x)))}");
        }
        for (int rid = 1; rid <= md.GetTableRowCount(TableIndex.Property); rid++)
        {
            var h = MetadataTokens.PropertyDefinitionHandle(rid);
            var e = md.GetPropertyDefinition(h);
            var a = e.GetAccessors();
            Out.WriteLine($"Property[{rid}] flags=0x{(ushort)e.Attributes:x4} name={md.GetString(e.Name)} sig={Try(() => p.MethodSig(e.DecodeSignature(p, null)))} get={Tok(a.Getter)} set={Tok(a.Setter)} others={string.Join(",", a.Others.Select(x => Rid(x)))}");
            Constant(md, e.GetDefaultValue());
        }
        for (int rid = 1; rid <= md.GetTableRowCount(TableIndex.MethodImpl); rid++)
        {
            var m = md.GetMethodImplementation(MetadataTokens.MethodImplementationHandle(rid));
            Out.WriteLine($"MethodImpl[{rid}] class={Tok(m.Type)} body={Try(() => p.Member(m.MethodBody))} decl={Try(() => p.Member(m.MethodDeclaration))}");
        }
        for (int rid = 1; rid <= md.GetTableRowCount(TableIndex.ModuleRef); rid++)
        {
            var m = md.GetModuleReference(MetadataTokens.ModuleReferenceHandle(rid));
            Out.WriteLine($"ModuleRef[{rid}] name={md.GetString(m.Name)}");
        }
        for (int rid = 1; rid <= md.GetTableRowCount(TableIndex.TypeSpec); rid++)
        {
            var h = MetadataTokens.TypeSpecificationHandle(rid);
            Out.WriteLine($"TypeSpec[{rid}] {Try(() => md.GetTypeSpecification(h).DecodeSignature(p, null))}");
        }
        if (md.IsAssembly)
        {
            var a = md.GetAssemblyDefinition();
            Out.WriteLine($"Assembly[1] hash=0x{(uint)a.HashAlgorithm:x} version={a.Version} flags=0x{(uint)a.Flags:x} key={Hex(md.GetBlobBytes(a.PublicKey))} name={md.GetString(a.Name)} culture={md.GetString(a.Culture)}");
        }
        foreach (var h in md.AssemblyReferences)
        {
            var a = md.GetAssemblyReference(h);
            Out.WriteLine($"AssemblyRef[{Rid(h)}] version={a.Version} flags=0x{(uint)a.Flags:x} key={Hex(md.GetBlobBytes(a.PublicKeyOrToken))} name={md.GetString(a.Name)} culture={md.GetString(a.Culture)} hash={Hex(md.GetBlobBytes(a.HashValue))}");
        }
        foreach (var h in md.AssemblyFiles)
        {
            var f = md.GetAssemblyFile(h);
            Out.WriteLine($"File[{Rid(h)}] metadata={f.ContainsMetadata} name={md.GetString(f.Name)} hash={Hex(md.GetBlobBytes(f.HashValue))}");
        }
        foreach (var h in md.ExportedTypes)
        {
            var e = md.GetExportedType(h);
            Out.WriteLine($"ExportedType[{Rid(h)}] flags=0x{(uint)e.Attributes:x8} forwarder={e.IsForwarder} typedef=0x{e.GetTypeDefinitionId():x} ns={md.GetString(e.Namespace)} name={md.GetString(e.Name)} impl={Tok(e.Implementation)}");
        }
        foreach (var h in md.ManifestResources)
        {
            var r = md.GetManifestResource(h);
            Out.WriteLine($"ManifestResource[{Rid(h)}] offset=0x{r.Offset:x} flags=0x{(uint)r.Attributes:x} name={md.GetString(r.Name)} impl={Tok(r.Implementation)}");
            if (r.Implementation.IsNil && pe != null)
            {
                Resource(pe, r);
            }
        }
        for (int rid = 1; rid <= md.GetTableRowCount(TableIndex.MethodSpec); rid++)
        {
            var h = MetadataTokens.MethodSpecificationHandle(rid);
            Out.WriteLine($"MethodSpec[{rid}] {Try(() => p.Member(h))}");
        }
        if (md.GetTableRowCount(TableIndex.Document) > 0 || md.DebugMetadataHeader != null)
        {
            DumpPdb(md, p);
        }
        var us = md.GetNextHandle(MetadataTokens.UserStringHandle(0));
        while (!us.IsNil)
        {
            Out.WriteLine($"US 0x{MetadataTokens.GetHeapOffset(us):x} {Quote(md.GetUserString(us))}");
            us = md.GetNextHandle(us);
        }
    }

    static void DumpPdb(MetadataReader md, Provider p)
    {
        foreach (var h in md.Documents)
        {
            var d = md.GetDocument(h);
            Out.WriteLine($"Document[{Rid(h)}] name={md.GetString(d.Name)} hashalg={G(md, d.HashAlgorithm)} hash={Hex(md.GetBlobBytes(d.Hash))} language={G(md, d.Language)}");
        }
        foreach (var h in md.MethodDebugInformation)
        {
            var m = md.GetMethodDebugInformation(h);
            Out.WriteLine($"MethodDebugInformation[{Rid(h)}] document={Tok(m.Document)} kickoff={Tok(m.GetStateMachineKickoffMethod())} localsig={Tok(m.LocalSignature)}");
            if (!m.SequencePointsBlob.IsNil)
            {
                foreach (var s in m.GetSequencePoints())
                {
                    Out.WriteLine(s.IsHidden
                        ? $"  sp IL_{s.Offset:x4} document={Tok(s.Document)} hidden"
                        : $"  sp IL_{s.Offset:x4} document={Tok(s.Document)} {s.StartLine}:{s.StartColumn}-{s.EndLine}:{s.EndColumn}");
                }
            }
        }
        foreach (var h in md.LocalScopes)
        {
            var s = md.GetLocalScope(h);
            Out.WriteLine($"LocalScope[{Rid(h)}] method={Tok(s.Method)} import={Tok(s.ImportScope)} start=0x{s.StartOffset:x} length=0x{s.Length:x} variables={Ranges(s.GetLocalVariables().Select(x => Rid(x)))} constants={Ranges(s.GetLocalConstants().Select(x => Rid(x)))}");
        }
        foreach (var h in md.LocalVariables)
        {
            var v = md.GetLocalVariable(h);
            Out.WriteLine($"LocalVariable[{Rid(h)}] flags=0x{(ushort)v.Attributes:x4} index={v.Index} name={md.GetString(v.Name)}");
        }
        foreach (var h in md.LocalConstants)
        {
            var v = md.GetLocalConstant(h);
            Out.WriteLine($"LocalConstant[{Rid(h)}] name={md.GetString(v.Name)} sig={Hex(md.GetBlobBytes(v.Signature))}");
        }
        foreach (var h in md.ImportScopes)
        {
            var v = md.GetImportScope(h);
            Out.WriteLine($"ImportScope[{Rid(h)}] parent={Tok(v.Parent)} imports={Hex(md.GetBlobBytes(v.ImportsBlob))}");
        }
        foreach (var h in md.CustomDebugInformation)
        {
            var v = md.GetCustomDebugInformation(h);
            Out.WriteLine($"CustomDebugInformation[{Rid(h)}] parent={Tok(v.Parent)} kind={G(md, v.Kind)} value={Hex(md.GetBlobBytes(v.Value))}");
        }
    }

    static void GenericParams(MetadataReader md, Provider p, GenericParameterHandleCollection list)
    {
        foreach (var g in list)
        {
            var gp = md.GetGenericParameter(g);
            Out.WriteLine($"  GenericParam[{Rid(g)}] index={gp.Index} flags=0x{(ushort)gp.Attributes:x4} name={md.GetString(gp.Name)} owner={Tok(gp.Parent)}");
            foreach (var c in gp.GetConstraints())
            {
                Out.WriteLine($"    GenericParamConstraint[{Rid(c)}] {p.TypeToken(md.GetGenericParameterConstraint(c).Type)}");
            }
        }
    }

    static void Constant(MetadataReader md, ConstantHandle h)
    {
        if (!h.IsNil)
        {
            var c = md.GetConstant(h);
            Out.WriteLine($"  const Constant[{Rid(h)}] type=0x{(byte)c.TypeCode:x2} value={Hex(md.GetBlobBytes(c.Value))}");
        }
    }

    static void Body(MetadataReader md, Provider p, PEReader pe, int rva)
    {
        MethodBodyBlock body;
        try
        {
            body = pe.GetMethodBody(rva);
        }
        catch (Exception)
        {
            Out.WriteLine("  body error");
            return;
        }
        Out.WriteLine($"  body maxstack={body.MaxStack} size={body.Size} localsig={Tok(body.LocalSignature)} initlocals={body.LocalVariablesInitialized}");
        foreach (var r in body.ExceptionRegions)
        {
            string what = r.Kind switch
            {
                ExceptionRegionKind.Catch => "catch " + p.TypeToken(r.CatchType),
                ExceptionRegionKind.Filter => $"filter IL_{r.FilterOffset:x4}",
                ExceptionRegionKind.Finally => "finally",
                _ => "fault",
            };
            Out.WriteLine($"  .try IL_{r.TryOffset:x4}+0x{r.TryLength:x} handler IL_{r.HandlerOffset:x4}+0x{r.HandlerLength:x} {what}");
        }
        var il = body.GetILBytes();
        int at = 0;
        while (at < il.Length)
        {
            int start = at;
            ushort value = il[at++];
            if (value == 0xFE)
            {
                if (at >= il.Length)
                {
                    Out.WriteLine($"  IL_{start:x4}: truncated");
                    return;
                }
                value = (ushort)(0xFE00 | il[at++]);
            }
            if (!OpcodeTable.TryGetValue(value, out var op))
            {
                Out.WriteLine($"  IL_{start:x4}: unknown 0x{value:x}");
                continue;
            }
            int Need(int n)
            {
                if (at + n > il.Length)
                {
                    throw new EndOfStreamException();
                }
                int a = at;
                at += n;
                return a;
            }
            string operand;
            try
            {
                operand = op.OperandType switch
                {
                    OperandType.InlineNone => null,
                    OperandType.ShortInlineVar => il[Need(1)].ToString(),
                    OperandType.InlineVar => BitConverter.ToUInt16(il, Need(2)).ToString(),
                    OperandType.ShortInlineI => ((sbyte)il[Need(1)]).ToString(),
                    OperandType.InlineI => BitConverter.ToInt32(il, Need(4)).ToString(),
                    OperandType.InlineI8 => BitConverter.ToInt64(il, Need(8)).ToString(),
                    OperandType.ShortInlineR => $"0x{BitConverter.ToUInt32(il, Need(4)):x8}",
                    OperandType.InlineR => $"0x{BitConverter.ToUInt64(il, Need(8)):x16}",
                    OperandType.ShortInlineBrTarget => Target((sbyte)il[Need(1)], at),
                    OperandType.InlineBrTarget => Target(BitConverter.ToInt32(il, Need(4)), at),
                    OperandType.InlineSwitch => Switch(il, ref at),
                    _ => p.Token(BitConverter.ToInt32(il, Need(4)), op.OperandType),
                };
            }
            catch (EndOfStreamException)
            {
                Out.WriteLine($"  IL_{start:x4}: {op.Name} truncated");
                return;
            }
            Out.WriteLine(operand == null ? $"  IL_{start:x4}: {op.Name}" : $"  IL_{start:x4}: {op.Name} {operand}");
        }
    }

    static string Target(int delta, int next) => $"IL_{next + delta:x4}";

    static string Switch(byte[] il, ref int at)
    {
        if (at + 4 > il.Length)
        {
            throw new EndOfStreamException();
        }
        uint n = BitConverter.ToUInt32(il, at);
        at += 4;
        if ((ulong)at + 4UL * n > (ulong)il.Length)
        {
            throw new EndOfStreamException();
        }
        int next = at + 4 * (int)n;
        var targets = new List<string>();
        for (int i = 0; i < n; i++)
        {
            targets.Add(Target(BitConverter.ToInt32(il, at), next));
            at += 4;
        }
        return "(" + string.Join(", ", targets) + ")";
    }

    static void Resource(PEReader pe, ManifestResource r)
    {
        var dir = pe.PEHeaders.CorHeader.ResourcesDirectory;
        int offset;
        if (!pe.PEHeaders.TryGetDirectoryOffset(dir, out offset))
        {
            Out.WriteLine("  data unreadable");
            return;
        }
        var image = pe.GetEntireImage().GetContent().ToArray();
        int at = offset + (int)r.Offset;
        int length = BitConverter.ToInt32(image, at);
        var data = image.Skip(at + 4).Take(length).ToArray();
        Out.WriteLine($"  data size=0x{length:x}");
        if (length < 4 || BitConverter.ToUInt32(data, 0) != 0xBEEFCACE)
        {
            return;
        }
        using var reader = new System.Resources.ResourceReader(new MemoryStream(data));
        var names = new List<string>();
        var e = reader.GetEnumerator();
        while (e.MoveNext())
        {
            names.Add((string)e.Key);
        }
        names.Sort(StringComparer.Ordinal);
        foreach (var name in names)
        {
            reader.GetResourceData(name, out string type, out byte[] bytes);
            Out.WriteLine($"  entry {Quote(name)} type={type} data={Hex(bytes)}");
        }
    }

    static string Attribute(MetadataReader md, Provider p, CustomAttribute c)
    {
        try
        {
            var v = c.DecodeValue(p);
            var s = "(" + string.Join(", ", v.FixedArguments.Select(a => p.Arg(a))) + ")";
            if (v.NamedArguments.Length > 0)
            {
                s += " {" + string.Join(", ", v.NamedArguments.Select(n =>
                    (n.Kind == CustomAttributeNamedArgumentKind.Field ? "field " : "property ") + n.Name + "=" + p.Arg(new CustomAttributeTypedArgument<string>(n.Type, n.Value)))) + "}";
            }
            return s;
        }
        catch (Exception)
        {
            return "undecodable " + Hex(md.GetBlobBytes(c.Value));
        }
    }

    static string Try(Func<string> f)
    {
        try
        {
            return f();
        }
        catch (Exception)
        {
            return "undecodable";
        }
    }

    static string TypeOrNil(Provider p, EntityHandle h) => h.IsNil ? "-" : p.TypeToken(h);

    static int Rid(EntityHandle h) => MetadataTokens.GetRowNumber(h);

    static string Tok(EntityHandle h) => $"0x{(h.IsNil ? 0 : MetadataTokens.GetToken(h)):x8}";
    static string Tok(MethodDefinitionHandle h) => Tok((EntityHandle)h);
    static string Tok(TypeDefinitionHandle h) => Tok((EntityHandle)h);
    static string Tok(ModuleReferenceHandle h) => Tok((EntityHandle)h);
    static string Tok(StandaloneSignatureHandle h) => Tok((EntityHandle)h);
    static string Tok(DocumentHandle h) => Tok((EntityHandle)h);
    static string Tok(ImportScopeHandle h) => Tok((EntityHandle)h);

    static string G(MetadataReader md, GuidHandle h) => h.IsNil ? "null" : md.GetGuid(h).ToString("D");

    static string Hex(IEnumerable<byte> b)
    {
        if (b == null)
        {
            return "";
        }
        var sb = new StringBuilder();
        foreach (var x in b)
        {
            sb.Append(x.ToString("x2"));
        }
        return sb.ToString();
    }

    static string Ranges(IEnumerable<int> rids)
    {
        var parts = new List<string>();
        int start = -1, last = -1;
        foreach (var r in rids)
        {
            if (start >= 0 && r == last + 1)
            {
                last = r;
                continue;
            }
            if (start >= 0)
            {
                parts.Add(start == last ? $"{start}" : $"{start}-{last}");
            }
            start = last = r;
        }
        if (start >= 0)
        {
            parts.Add(start == last ? $"{start}" : $"{start}-{last}");
        }
        return parts.Count == 0 ? "-" : string.Join(",", parts);
    }

    public static string Quote(string s)
    {
        if (s == null)
        {
            return "null";
        }
        var sb = new StringBuilder("\"");
        foreach (char c in s)
        {
            switch (c)
            {
                case '"': sb.Append("\\\""); break;
                case '\\': sb.Append("\\\\"); break;
                case '\n': sb.Append("\\n"); break;
                case '\r': sb.Append("\\r"); break;
                case '\t': sb.Append("\\t"); break;
                case '\0': sb.Append("\\0"); break;
                default:
                    if (c < 0x20 || c > 0x7E)
                    {
                        sb.Append($"\\u{(int)c:x4}");
                    }
                    else
                    {
                        sb.Append(c);
                    }
                    break;
            }
        }
        return sb.Append('"').ToString();
    }

    // Signatures rendered as text (turbonop-clr's rendering).
    sealed class Provider : ISignatureTypeProvider<string, object>, ICustomAttributeTypeProvider<string>
    {
        readonly MetadataReader md;
        // Types this module defines that are enums: their names (as
        // signatures and serialized names spell them) and underlying types.
        readonly Dictionary<string, PrimitiveTypeCode> enums = new();

        public Provider(MetadataReader md)
        {
            this.md = md;
            foreach (var h in md.TypeDefinitions)
            {
                var t = md.GetTypeDefinition(h);
                foreach (var f in t.GetFields())
                {
                    var fd = md.GetFieldDefinition(f);
                    if ((fd.Attributes & FieldAttributes.Static) == 0 && md.GetString(fd.Name) == "value__")
                    {
                        PrimitiveTypeCode code;
                        try
                        {
                            code = Underlying(md.GetBlobReader(fd.Signature));
                        }
                        catch (Exception)
                        {
                            break;
                        }
                        enums[Name(h)] = code;
                        enums["valuetype " + Name(h)] = code;
                        enums["class " + Name(h)] = code;
                        enums[SerializedName(h)] = code;
                        break;
                    }
                }
            }
        }

        static PrimitiveTypeCode Underlying(BlobReader r)
        {
            r.ReadSignatureHeader();
            return (SignatureTypeCode)r.ReadCompressedInteger() switch
            {
                SignatureTypeCode.Boolean => PrimitiveTypeCode.Boolean,
                SignatureTypeCode.Char => PrimitiveTypeCode.Char,
                SignatureTypeCode.SByte => PrimitiveTypeCode.SByte,
                SignatureTypeCode.Byte => PrimitiveTypeCode.Byte,
                SignatureTypeCode.Int16 => PrimitiveTypeCode.Int16,
                SignatureTypeCode.UInt16 => PrimitiveTypeCode.UInt16,
                SignatureTypeCode.Int32 => PrimitiveTypeCode.Int32,
                SignatureTypeCode.UInt32 => PrimitiveTypeCode.UInt32,
                SignatureTypeCode.Int64 => PrimitiveTypeCode.Int64,
                SignatureTypeCode.UInt64 => PrimitiveTypeCode.UInt64,
                _ => throw new BadImageFormatException(),
            };
        }

        string SerializedName(TypeDefinitionHandle h)
        {
            var t = md.GetTypeDefinition(h);
            var enclosing = t.GetDeclaringType();
            if (!enclosing.IsNil)
            {
                return SerializedName(enclosing) + "+" + md.GetString(t.Name);
            }
            var ns = md.GetString(t.Namespace);
            return ns.Length == 0 ? md.GetString(t.Name) : ns + "." + md.GetString(t.Name);
        }

        public string Name(TypeDefinitionHandle h)
        {
            var t = md.GetTypeDefinition(h);
            var enclosing = t.GetDeclaringType();
            if (!enclosing.IsNil)
            {
                return Name(enclosing) + "/" + md.GetString(t.Name);
            }
            var ns = md.GetString(t.Namespace);
            return ns.Length == 0 ? md.GetString(t.Name) : ns + "." + md.GetString(t.Name);
        }

        public string Name(TypeReferenceHandle h)
        {
            var t = md.GetTypeReference(h);
            var ns = md.GetString(t.Namespace);
            var name = ns.Length == 0 ? md.GetString(t.Name) : ns + "." + md.GetString(t.Name);
            var scope = t.ResolutionScope;
            if (scope.IsNil)
            {
                return name;
            }
            switch (scope.Kind)
            {
                case HandleKind.AssemblyReference:
                    return "[" + md.GetString(md.GetAssemblyReference((AssemblyReferenceHandle)scope).Name) + "]" + name;
                case HandleKind.ModuleReference:
                    return "[.module " + md.GetString(md.GetModuleReference((ModuleReferenceHandle)scope).Name) + "]" + name;
                case HandleKind.TypeReference:
                    return Name((TypeReferenceHandle)scope) + "/" + md.GetString(t.Name);
                default:
                    return name;
            }
        }

        // A TypeDef, TypeRef, or TypeSpec token's type, with no class or
        // valuetype.
        public string TypeToken(EntityHandle h)
        {
            if (h.IsNil)
            {
                return "-";
            }
            return h.Kind switch
            {
                HandleKind.TypeDefinition => Name((TypeDefinitionHandle)h),
                HandleKind.TypeReference => Name((TypeReferenceHandle)h),
                HandleKind.TypeSpecification => Try(() => md.GetTypeSpecification((TypeSpecificationHandle)h).DecodeSignature(this, null)),
                _ => Tok(h),
            };
        }

        public string Token(int token, OperandType kind)
        {
            EntityHandle h;
            if (kind == OperandType.InlineString)
            {
                if ((token >> 24) != 0x70)
                {
                    return $"0x{token:x8}";
                }
                try
                {
                    return Quote(md.GetUserString(MetadataTokens.UserStringHandle(token & 0xFFFFFF)));
                }
                catch (Exception)
                {
                    return $"0x{token:x8}";
                }
            }
            try
            {
                h = MetadataTokens.EntityHandle(token);
            }
            catch (Exception)
            {
                return $"0x{token:x8}";
            }
            if (MetadataTokens.GetRowNumber(h) == 0 || MetadataTokens.GetRowNumber(h) > md.GetTableRowCount((TableIndex)(token >> 24)))
            {
                return $"0x{token:x8}";
            }
            return Try(() => h.Kind switch
            {
                HandleKind.TypeDefinition or HandleKind.TypeReference or HandleKind.TypeSpecification => TypeToken(h),
                HandleKind.MethodDefinition or HandleKind.MethodSpecification =>
                    (kind == OperandType.InlineTok ? "method " : "") + Member(h),
                HandleKind.FieldDefinition => (kind == OperandType.InlineTok ? "field " : "") + Member(h),
                HandleKind.MemberReference => (kind == OperandType.InlineTok
                    ? (md.GetMemberReference((MemberReferenceHandle)h).GetKind() == MemberReferenceKind.Field ? "field " : "method ")
                    : "") + Member(h),
                HandleKind.StandaloneSignature => MethodSig(md.GetStandaloneSignature((StandaloneSignatureHandle)h).DecodeMethodSignature(this, null)),
                _ => $"0x{token:x8}",
            });
        }

        string Parent(EntityHandle h) => h.Kind switch
        {
            HandleKind.ModuleReference => "[.module " + md.GetString(md.GetModuleReference((ModuleReferenceHandle)h).Name) + "]",
            HandleKind.MethodDefinition => TypeToken(md.GetMethodDefinition((MethodDefinitionHandle)h).GetDeclaringType()),
            _ => TypeToken(h),
        };

        // A method or field, as IL names it.
        public string Member(EntityHandle h)
        {
            switch (h.Kind)
            {
                case HandleKind.MethodDefinition:
                {
                    var m = md.GetMethodDefinition((MethodDefinitionHandle)h);
                    return MemberSig(m.DecodeSignature(this, null), Name(m.GetDeclaringType()), md.GetString(m.Name));
                }
                case HandleKind.FieldDefinition:
                {
                    var f = md.GetFieldDefinition((FieldDefinitionHandle)h);
                    return f.DecodeSignature(this, null) + " " + Name(f.GetDeclaringType()) + "::" + md.GetString(f.Name);
                }
                case HandleKind.MemberReference:
                {
                    var r = md.GetMemberReference((MemberReferenceHandle)h);
                    var parent = Parent(r.Parent);
                    if (r.GetKind() == MemberReferenceKind.Field)
                    {
                        return r.DecodeFieldSignature(this, null) + " " + parent + "::" + md.GetString(r.Name);
                    }
                    return MemberSig(r.DecodeMethodSignature(this, null), parent, md.GetString(r.Name));
                }
                case HandleKind.MethodSpecification:
                {
                    var s = md.GetMethodSpecification((MethodSpecificationHandle)h);
                    return Member(s.Method) + "<" + string.Join(", ", s.DecodeSignature(this, null)) + ">";
                }
                default:
                    return Tok(h);
            }
        }

        static string Prefix(SignatureHeader h, int generic)
        {
            var s = "";
            if (h.IsInstance)
            {
                s += "instance ";
            }
            if (h.HasExplicitThis)
            {
                s += "explicit ";
            }
            s += h.CallingConvention switch
            {
                SignatureCallingConvention.Default => "",
                SignatureCallingConvention.CDecl => "unmanaged cdecl ",
                SignatureCallingConvention.StdCall => "unmanaged stdcall ",
                SignatureCallingConvention.ThisCall => "unmanaged thiscall ",
                SignatureCallingConvention.FastCall => "unmanaged fastcall ",
                SignatureCallingConvention.VarArgs => "vararg ",
                SignatureCallingConvention.Unmanaged => "unmanaged ",
                _ => $"callconv({(int)h.CallingConvention}) ",
            };
            if (generic > 0)
            {
                s += $"generic<{generic}> ";
            }
            return s;
        }

        static string Params(MethodSignature<string> m)
        {
            var list = m.ParameterTypes.ToList();
            if (m.RequiredParameterCount < list.Count)
            {
                list.Insert(m.RequiredParameterCount, "...");
            }
            return "(" + string.Join(", ", list) + ")";
        }

        static string MemberSig(MethodSignature<string> m, string parent, string name) =>
            Prefix(m.Header, m.GenericParameterCount) + m.ReturnType + " " + parent + "::" + name + Params(m);

        public string MethodSig(MethodSignature<string> m) =>
            Prefix(m.Header, m.GenericParameterCount) + m.ReturnType + " " + Params(m);

        public string Arg(CustomAttributeTypedArgument<string> a) => a.Type + "(" + Value(a.Value) + ")";

        string Value(object v)
        {
            switch (v)
            {
                case null: return "null";
                case bool b: return b ? "true" : "false";
                case char c: return ((int)c).ToString();
                case float f: return $"0x{BitConverter.SingleToUInt32Bits(f):x8}";
                case double d: return $"0x{BitConverter.DoubleToUInt64Bits(d):x16}";
                case string s: return Quote(s);
                case ImmutableArray<CustomAttributeTypedArgument<string>> a:
                    return "[" + string.Join(", ", a.Select(Arg)) + "]";
                default: return Convert.ToString(v, System.Globalization.CultureInfo.InvariantCulture);
            }
        }

        static string Kind(byte raw) => raw switch
        {
            0x11 => "valuetype ",
            0x12 => "class ",
            _ => "",
        };

        public string GetArrayType(string elementType, ArrayShape shape)
        {
            var dims = new List<string>();
            for (int i = 0; i < shape.Rank; i++)
            {
                bool hasLo = i < shape.LowerBounds.Length, hasSize = i < shape.Sizes.Length;
                int lo = hasLo ? shape.LowerBounds[i] : 0;
                if (hasSize)
                {
                    dims.Add($"{lo}...{(long)lo + shape.Sizes[i] - 1}");
                }
                else if (hasLo)
                {
                    dims.Add($"{lo}...");
                }
                else
                {
                    dims.Add("");
                }
            }
            return elementType + "[" + string.Join(",", dims) + "]";
        }

        public string GetByReferenceType(string elementType) => elementType + "&";
        public string GetFunctionPointerType(MethodSignature<string> signature) =>
            "method " + Prefix(signature.Header, signature.GenericParameterCount) + signature.ReturnType + " *" + Params(signature);
        public string GetGenericInstantiation(string genericType, ImmutableArray<string> typeArguments) =>
            genericType + "<" + string.Join(", ", typeArguments) + ">";
        public string GetGenericMethodParameter(object genericContext, int index) => "!!" + index;
        public string GetGenericTypeParameter(object genericContext, int index) => "!" + index;
        public string GetModifiedType(string modifier, string unmodifiedType, bool isRequired) =>
            unmodifiedType + (isRequired ? " modreq(" : " modopt(") + modifier + ")";
        public string GetPinnedType(string elementType) => elementType + " pinned";
        public string GetPointerType(string elementType) => elementType + "*";
        public string GetPrimitiveType(PrimitiveTypeCode typeCode) => typeCode switch
        {
            PrimitiveTypeCode.Void => "void",
            PrimitiveTypeCode.Boolean => "bool",
            PrimitiveTypeCode.Char => "char",
            PrimitiveTypeCode.SByte => "int8",
            PrimitiveTypeCode.Byte => "uint8",
            PrimitiveTypeCode.Int16 => "int16",
            PrimitiveTypeCode.UInt16 => "uint16",
            PrimitiveTypeCode.Int32 => "int32",
            PrimitiveTypeCode.UInt32 => "uint32",
            PrimitiveTypeCode.Int64 => "int64",
            PrimitiveTypeCode.UInt64 => "uint64",
            PrimitiveTypeCode.Single => "float32",
            PrimitiveTypeCode.Double => "float64",
            PrimitiveTypeCode.String => "string",
            PrimitiveTypeCode.Object => "object",
            PrimitiveTypeCode.IntPtr => "native int",
            PrimitiveTypeCode.UIntPtr => "native uint",
            PrimitiveTypeCode.TypedReference => "typedref",
            _ => "?",
        };
        public string GetSZArrayType(string elementType) => elementType + "[]";
        public string GetTypeFromDefinition(MetadataReader reader, TypeDefinitionHandle handle, byte rawTypeKind) =>
            Kind(rawTypeKind) + Name(handle);
        public string GetTypeFromReference(MetadataReader reader, TypeReferenceHandle handle, byte rawTypeKind) =>
            Kind(rawTypeKind) + Name(handle);
        public string GetTypeFromSpecification(MetadataReader reader, object genericContext, TypeSpecificationHandle handle, byte rawTypeKind) =>
            reader.GetTypeSpecification(handle).DecodeSignature(this, genericContext);

        public string GetSystemType() => "System.Type";
        public string GetTypeFromSerializedName(string name) => name;
        public PrimitiveTypeCode GetUnderlyingEnumType(string type)
        {
            if (enums.TryGetValue(type, out var code))
            {
                return code;
            }
            var comma = type.IndexOf(',');
            if (comma >= 0 && enums.TryGetValue(type.Substring(0, comma).Trim(), out code))
            {
                return code;
            }
            return PrimitiveTypeCode.Int32;
        }
        public bool IsSystemType(string type) =>
            type == "System.Type" || (type.StartsWith("[") && type.EndsWith("]System.Type"));
    }
}
