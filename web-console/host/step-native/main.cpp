#include "step-transfer.hxx"

#include <BRepBndLib.hxx>
#include <BRepMesh_IncrementalMesh.hxx>
#include <BRep_Tool.hxx>
#include <BRepTools.hxx>
#include <Bnd_Box.hxx>
#include <TColStd_IndexedDataMapOfStringString.hxx>
#include <OSD_Parallel.hxx>
#include <OSD_ThreadPool.hxx>
#include <Precision.hxx>
#include <Poly_Triangulation.hxx>
#include <RWGltf_CafWriter.hxx>
#include <STEPCAFControl_Reader.hxx>
#include <STEPConstruct_ExternRefs.hxx>
#include <Standard_Failure.hxx>
#include <Standard_Version.hxx>
#include <TDocStd_Document.hxx>
#include <TopExp_Explorer.hxx>
#include <TopoDS.hxx>
#include <XCAFDoc_DocumentTool.hxx>
#include <XCAFDoc_ShapeTool.hxx>

#include <chrono>
#include <fstream>
#include <iostream>
#include <stdexcept>
#include <string>
#include <vector>
#include <sys/resource.h>

using Clock = std::chrono::steady_clock;
static_assert(OCC_VERSION_MAJOR == 7 && OCC_VERSION_MINOR == 6 && OCC_VERSION_MAINTENANCE == 1,
              "Build with the pinned OCCT 7.6.1 used by the bulk-lexer WASM runtime");
static double milliseconds(Clock::time_point start) {
    return std::chrono::duration<double, std::milli>(Clock::now() - start).count();
}

class InputBuffer : public std::streambuf {
public:
    explicit InputBuffer(std::vector<char>& bytes) {
        setg(bytes.data(), bytes.data(), bytes.data() + bytes.size());
    }
};

static void boundProcess() {
    const rlimit memory{4ULL << 30, 4ULL << 30};
    const rlimit fileSize{100ULL << 20, 100ULL << 20};
    if (setrlimit(RLIMIT_AS, &memory) || setrlimit(RLIMIT_FSIZE, &fileSize))
        throw std::runtime_error("Cannot apply conversion limits");
}

int main(int argc, char** argv) {
    try {
        if (argc != 2 || (std::string(argv[1]) != "1" && std::string(argv[1]) != "8"))
            throw std::runtime_error("Expected worker thread count 1 or 8");
        boundProcess();
        const int threads = std::stoi(argv[1]);
        OSD_Parallel::SetUseOcctThreads(true);
        OSD_ThreadPool::DefaultPool(threads)->SetNbDefaultThreadsToLaunch(threads);

        auto start = Clock::now();
        std::ifstream input("/proc/self/fd/3", std::ios::binary | std::ios::ate);
        const auto size = input.tellg();
        if (size <= 0 || size > (200LL << 20)) throw std::runtime_error("Invalid STEP source size");
        std::vector<char> bytes(static_cast<size_t>(size));
        input.seekg(0);
        if (!input.read(bytes.data(), bytes.size())) throw std::runtime_error("Cannot read STEP source");
        const double readMs = milliseconds(start);

        start = Clock::now();
        STEPCAFControl_Reader step;
        step.SetNameMode(true);
        step.SetColorMode(true);
        auto& reader = step.ChangeReader();
        {
            InputBuffer buffer(bytes);
            std::istream stream(&buffer);
            if (reader.ReadStream("source.step", stream) != IFSelect_RetDone)
                throw std::runtime_error("Cannot parse STEP source");
        }
        std::vector<char>().swap(bytes);
        const double parseMs = milliseconds(start);

        start = Clock::now();
        // Transfer otherwise follows STEP external file references with OS file access.
        STEPConstruct_ExternRefs references(reader.WS());
        references.LoadExternRefs();
        if (references.NbExternRefs() != 0) return 3;
        Handle(TDocStd_Document) document = new TDocStd_Document("BinXCAF");
        if (!step.Transfer(document)) throw std::runtime_error("Cannot transfer STEP source");
        verifyStepTransfer(reader);
        const double transferMs = milliseconds(start);

        start = Clock::now();
        TDF_LabelSequence shapes;
        XCAFDoc_DocumentTool::ShapeTool(document->Main())->GetFreeShapes(shapes);
        int roots = 0;
        for (int index = 1; index <= shapes.Length(); ++index) {
            const TDF_Label& label = shapes.Value(index);
            const auto shape = XCAFDoc_ShapeTool::GetShape(label);
            if (shape.IsNull()) throw std::runtime_error("Missing STEP root shape");
            if (!TopExp_Explorer(shape, TopAbs_FACE).More()) continue;
            Bnd_Box bounds;
            BRepBndLib::Add(shape, bounds, false);
            if (bounds.IsVoid()) throw std::runtime_error("Missing STEP root bounds");
            double minX, minY, minZ, maxX, maxY, maxZ;
            bounds.Get(minX, minY, minZ, maxX, maxY, maxZ);
            const double span = (maxX - minX + maxY - minY + maxZ - minZ) / 3;
            const double scaled = span * 0.001;
            const double deflection = scaled < Precision::Confusion() ? 1.0 : scaled;
            BRepTools::Clean(shape);
            BRepMesh_IncrementalMesh mesh(shape, deflection, false, 0.5, threads > 1);
            if (!mesh.IsDone()) throw std::runtime_error("Cannot triangulate STEP shape");
            for (TopExp_Explorer faces(shape, TopAbs_FACE); faces.More(); faces.Next()) {
                TopLoc_Location location;
                const auto triangulation = BRep_Tool::Triangulation(TopoDS::Face(faces.Current()), location);
                if (triangulation.IsNull() || triangulation->NbTriangles() == 0)
                    throw std::runtime_error("Incomplete STEP triangulation");
            }
            ++roots;
        }
        if (roots == 0) throw std::runtime_error("No shapes to triangulate");
        const double meshMs = milliseconds(start);

        start = Clock::now();
        RWGltf_CafWriter writer("preview.glb", true);
        writer.ChangeCoordinateSystemConverter().SetInputLengthUnit(0.001);
        writer.ChangeCoordinateSystemConverter().SetInputCoordinateSystem(RWMesh_CoordinateSystem_Yup);
        writer.SetMergeFaces(true);
        TColStd_IndexedDataMapOfStringString info;
        if (!writer.Perform(document, info, Message_ProgressRange()))
            throw std::runtime_error("Cannot write GLB");
        const double encodeMs = milliseconds(start);
        rusage usage{};
        if (getrusage(RUSAGE_SELF, &usage)) throw std::runtime_error("Cannot measure process memory");
        std::ofstream metrics("timing.json");
        metrics << "{\"occt\":\"" << OCC_VERSION_COMPLETE << "\",\"threads\":" << threads
                << ",\"readMs\":" << readMs << ",\"parseMs\":" << parseMs
                << ",\"transferMs\":" << transferMs << ",\"meshMs\":" << meshMs
                << ",\"encodeMs\":" << encodeMs << ",\"roots\":" << roots
                << ",\"peakRssKiB\":" << usage.ru_maxrss << "}";
        if (!metrics) throw std::runtime_error("Cannot write conversion report");
    } catch (const Standard_Failure& error) {
        std::cerr << error.GetMessageString() << '\n';
        return 1;
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
