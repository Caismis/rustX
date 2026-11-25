#ifdef EMSCRIPTEN

#include "js-interface.hpp"
#include "importer-step.hpp"
#include "importer-iges.hpp"
#include "importer-brep.hpp"
#include "importer-utils.hpp"
#include "native-profile.hpp"
#include <emscripten/bind.h>

#include <array>

// JS owns the complete double buffers. Only this fixed stack chunk is exposed
// transiently; TypedArray.set copies it before the next allocation/heap growth.
class NumericArrayWriter
{
public:
    NumericArrayWriter (emscripten::val& array) : mArray (array), mSize (0), mOffset (0) {
        // Initialize the Embind invoker before exposing a WASM memory view.
        mArray.call<void> ("set", emscripten::val::global ("Float64Array").new_ (0), 0);
    }
    void Append (double value) {
        mValues[mSize++] = value;
        if (mSize == mValues.size ()) Flush ();
    }
    void Flush () {
        if (!mSize) return;
        emscripten::val view (emscripten::typed_memory_view (mSize, mValues.data ()));
        mArray.call<void> ("set", view, mOffset);
        mOffset += mSize; mSize = 0;
    }
private:
    emscripten::val& mArray;
    size_t mSize, mOffset;
    std::array<double, 4096> mValues;
};

struct CadOutputLimitExceeded {};

class HierarchyWriter
{
public:
    HierarchyWriter (emscripten::val& meshesArr) :
        mMeshesArr (meshesArr),
        mMeshCount (0), mVertices (0), mTriangles (0), mOutputBytes (0), mNodes (0), mFaces (0)
    {
    }

    void WriteNode (const NodePtr& node, emscripten::val& nodeObj)
    {
        if (++mNodes > 512) throw CadOutputLimitExceeded ();
        nodeObj.set ("name", node->GetName ());

        emscripten::val nodeMeshesArr (emscripten::val::array ());
        WriteMeshes (node, nodeMeshesArr);
        nodeObj.set ("meshes", nodeMeshesArr);

        std::vector<NodePtr> children = node->GetChildren ();
        emscripten::val childrenArr (emscripten::val::array ());
        for (int childIndex = 0; childIndex < children.size (); childIndex++) {
            const NodePtr& child = children[childIndex];
            emscripten::val childNodeObj (emscripten::val::object ());
            WriteNode (child, childNodeObj);
            childrenArr.set (childIndex, childNodeObj);
        }
        nodeObj.set ("children", childrenArr);
    }

private:
    void WriteMeshes (const NodePtr& node, emscripten::val& nodeMeshesArr)
    {
        if (!node->IsMeshNode ()) {
            return;
        }

        int nodeMeshCount = 0;
        node->EnumerateMeshes ([&](const Mesh& mesh) {
            emscripten::val meshObj (emscripten::val::object ());
            meshObj.set ("name", mesh.GetName ());

            int vertexCount = 0;
            int normalCount = 0;
            int triangleCount = 0;
            int brepFaceCount = 0;

            // Count and admit before allocating JS-owned output; no native
            // mesh-sized temporary and no boxed numeric arrays are retained.
            size_t positionsSize = 0, normalsSize = 0, indicesSize = 0;
            mesh.EnumerateFaces ([&](const Face& face) {
                if (++mFaces > 1000000) throw CadOutputLimitExceeded ();
                // Admit before arithmetic: size_t is 32-bit in this WASM.
                // Repeated/shared topology can expose huge logical streams
                // without requiring a correspondingly huge native shape.
                const size_t vertices = face.VertexCount (), normals = face.NormalCount (), triangles = face.TriangleCount ();
                if (vertices > 13000000 - mVertices - positionsSize / 3
                    || normals > 13000000 - normalsSize / 3
                    || triangles > 5000000 - mTriangles - indicesSize / 3)
                    throw CadOutputLimitExceeded ();
                positionsSize += vertices * 3;
                normalsSize += normals * 3;
                indicesSize += triangles * 3;
            });
            mVertices += positionsSize / 3; mTriangles += indicesSize / 3;
            mOutputBytes += (positionsSize + normalsSize + indicesSize) * sizeof (double);
            if (mMeshCount >= 256 || mVertices > 13000000 || mTriangles > 5000000 || mOutputBytes > 1024ULL * 1024 * 1024)
                throw CadOutputLimitExceeded ();
            emscripten::val float64 = emscripten::val::global ("Float64Array");
            emscripten::val positionArr = float64.new_ (positionsSize);
            emscripten::val normalArr = float64.new_ (normalsSize);
            emscripten::val indexArr = float64.new_ (indicesSize);
            emscripten::val brepFaceArr (emscripten::val::array ());
            NumericArrayWriter positions (positionArr), normals (normalArr), indices (indexArr);

            mesh.EnumerateFaces ([&](const Face& face) {
                int triangleOffset = triangleCount;
                int vertexOffset = vertexCount;
                face.EnumerateVertices ([&](double x, double y, double z) {
                    positions.Append (x);
                    positions.Append (y);
                    positions.Append (z);
                    vertexCount += 1;
                });
                face.EnumerateNormals ([&](double x, double y, double z) {
                    normals.Append (x);
                    normals.Append (y);
                    normals.Append (z);
                    normalCount += 1;
                });
                face.EnumerateTriangles ([&](int v0, int v1, int v2) {
                    indices.Append (static_cast<double> (vertexOffset) + static_cast<double> (v0));
                    indices.Append (static_cast<double> (vertexOffset) + static_cast<double> (v1));
                    indices.Append (static_cast<double> (vertexOffset) + static_cast<double> (v2));
                    triangleCount += 1;
                });
                emscripten::val brepFaceObj (emscripten::val::object ());
                brepFaceObj.set ("first", triangleOffset);
                brepFaceObj.set ("last", triangleCount - 1);
                Color faceColor;
                if (face.GetColor (faceColor)) {
                    emscripten::val colorArr (emscripten::val::array ());
                    colorArr.set (0, faceColor.r);
                    colorArr.set (1, faceColor.g);
                    colorArr.set (2, faceColor.b);
                    brepFaceObj.set ("color", colorArr);
                } else {
                    brepFaceObj.set ("color", emscripten::val::null ());
                }
                brepFaceArr.set (brepFaceCount, brepFaceObj);
                brepFaceCount += 1;
            });

            positions.Flush (); normals.Flush (); indices.Flush ();

            emscripten::val attributesObj (emscripten::val::object ());

            emscripten::val positionObj (emscripten::val::object ());
            positionObj.set ("array", positionArr);
            attributesObj.set ("position", positionObj);

            if (vertexCount == normalCount) {
                emscripten::val normalObj (emscripten::val::object ());
                normalObj.set ("array", normalArr);
                attributesObj.set ("normal", normalObj);
            }

            emscripten::val indexObj (emscripten::val::object ());
            indexObj.set ("array", indexArr);

            meshObj.set ("attributes", attributesObj);
            meshObj.set ("index", indexObj);

            Color meshColor;
            if (mesh.GetColor (meshColor)) {
                emscripten::val colorArr (emscripten::val::array ());
                colorArr.set (0, meshColor.r);
                colorArr.set (1, meshColor.g);
                colorArr.set (2, meshColor.b);
                meshObj.set ("color", colorArr);
            }

            meshObj.set ("brep_faces", brepFaceArr);

            mMeshesArr.set (mMeshCount, meshObj);
            nodeMeshesArr.set (nodeMeshCount, mMeshCount);
            mMeshCount += 1;
            nodeMeshCount += 1;
        });
    }

    emscripten::val& mMeshesArr;
    int mMeshCount;
    size_t mVertices, mTriangles, mOutputBytes, mNodes, mFaces;
};

// Admission happens before any JavaScript mesh arrays are allocated. Exceeding
// a limit discards the whole optional tier, never an incomplete set of parts.
struct LodBudgetExceeded {};

class LodBudget
{
public:
    LodBudget () : bytes (0), triangles (0), meshes (0), nodes (0) {}

    void AddBytes (size_t amount)
    {
        if (amount > 8 * 1024 * 1024 - bytes) throw LodBudgetExceeded ();
        bytes += amount;
    }

    void AddName (size_t overhead, size_t length)
    {
        if (length > (8 * 1024 * 1024 - bytes) / 4) throw LodBudgetExceeded ();
        AddBytes (overhead + length * 4);
    }

    void Check (const NodePtr& node)
    {
        if (++nodes > 512) {
            throw LodBudgetExceeded ();
        }
        AddName (256, node->GetName ().size ());
        if (node->IsMeshNode ()) {
            node->EnumerateMeshes ([&](const Mesh& mesh) {
                if (++meshes > 256) {
                    throw LodBudgetExceeded ();
                }
                AddName (512, mesh.GetName ().size ());
                mesh.EnumerateFaces ([&](const Face& face) {
                    AddBytes (128);
                    // Count without transforming every vertex a second time.
                    const size_t vertexCount = face.VertexCount (), triangleCount = face.TriangleCount ();
                    if (vertexCount > (8 * 1024 * 1024 - bytes) / (6 * sizeof (double))
                        || triangleCount > 65536 - triangles) throw LodBudgetExceeded ();
                    AddBytes (vertexCount * 6 * sizeof (double));
                    triangles += triangleCount;
                    AddBytes (triangleCount * 3 * sizeof (double));
                });
            });
        }
        for (const NodePtr& child : node->GetChildren ()) {
            Check (child);
        }
    }

private:
    size_t bytes;
    size_t triangles;
    size_t meshes;
    size_t nodes;
};

static emscripten::val WriteResult (const NodePtr& rootNode)
{
    NativeSpan span (NativePhase::Export);
    emscripten::val result (emscripten::val::object ());
    emscripten::val root (emscripten::val::object ());
    emscripten::val meshes (emscripten::val::array ());
    HierarchyWriter writer (meshes);
    try {
        writer.WriteNode (rootNode, root);
    } catch (const CadOutputLimitExceeded&) {
        result.set ("success", false); result.set ("error", "limit"); return result;
    }
    result.set ("success", true);
    result.set ("root", root);
    result.set ("meshes", meshes);
    return result;
}

static emscripten::val ImportFile (ImporterPtr importer, const emscripten::val& buffer, const ImportParams& finalParams, const emscripten::val& options)
{
    emscripten::val onLod (emscripten::val::undefined ());
    if (!options.isNull () && !options.isUndefined () && options.hasOwnProperty ("onLod")) {
        onLod = options["onLod"];
    }
    bool reportLod = onLod.typeOf ().as<std::string> () == "function";
    emscripten::val onProfile (emscripten::val::undefined ());
    if (!options.isNull () && !options.isUndefined () && options.hasOwnProperty ("onNativeProfile")) onProfile = options["onNativeProfile"];
    NativeProfile profile;
    struct ProfileLifetime { ~ProfileLifetime () { CurrentNativeProfile () = nullptr; } } profileLifetime;
    const bool profiling = onProfile.typeOf ().as<std::string> () == "function";
    CurrentNativeProfile () = profiling ? &profile : nullptr;
    const double nativeStart = profiling ? emscripten_get_now () : 0;
    ImportParams params = finalParams;
    params.shapeSizes = std::make_shared<ShapeSizeCache> ();
    const std::shared_ptr<ShapeSizeCache> shapeSizes = params.shapeSizes;
    int lodCount = 2;
    if (!options.isNull () && !options.isUndefined () && options.hasOwnProperty ("lodTiers")) lodCount = options["lodTiers"].as<int> ();
    if (lodCount != 1 && lodCount != 2) reportLod = false;

    if (reportLod) {
        params.linearDeflectionType = ImportParams::LinearDeflectionType::BoundingBoxRatio;
        params.linearDeflection = 0.02;
        params.angularDeflection = 1.0;
    }

    std::vector<uint8_t> bufferArr;
    { NativeSpan span (NativePhase::InputCopy); bufferArr = emscripten::convertJSArrayToNumberVector<std::uint8_t> (buffer); }
    if (!options.isNull () && !options.isUndefined () && options.hasOwnProperty ("onInputCopied")) {
        emscripten::val copied = options["onInputCopied"];
        if (copied.typeOf ().as<std::string> () == "function") {
            copied ();
        }
    }
    Importer::Result importResult = importer->LoadFile (bufferArr, params);
    std::vector<uint8_t> ().swap (bufferArr);
    if (importResult != Importer::Result::Success) {
        emscripten::val result (emscripten::val::object ());
        result.set ("success", false);
        return result;
    }

    // Embedded BREP meshes are part of the source's original final result.
    // Keep them intact instead of replacing them during optional refinement.
    ImporterBrep* brep = dynamic_cast<ImporterBrep*> (importer.get ());
    if (brep != nullptr && brep->HasSourceTriangulation ()) {
        reportLod = false;
        params = finalParams;
    }

    if (reportLod) {
        for (int level = 0; level < lodCount; level++) {
            profile.tier = level;
            if (level == 1) {
                params.linearDeflection = 0.005;
                params.angularDeflection = 0.75;
            }
            NodePtr rootNode = importer->GetRootNode ();
            try {
                LodBudget budget;
                { NativeSpan span (NativePhase::LodPreflight); budget.Check (rootNode); }
                // Preflight has tessellated every root. Reusing those native
                // triangulations during export avoids rebuilding the tier.
                params.resetTriangulation = false;
                emscripten::val lod = WriteResult (rootNode);
                if (profiling && level == 0) profile.Add (NativePhase::FirstCoarse, emscripten_get_now () - nativeStart);
                onLod (level, lod);
            } catch (const LodBudgetExceeded&) {
                // The final full result remains available even when a preview
                // cannot fit the independently bounded optional output.
            }
            params.resetTriangulation = true;
        }
        params = finalParams;
        params.shapeSizes = shapeSizes;
        params.resetTriangulation = true;
    }
    profile.tier = 2;
    emscripten::val result = WriteResult (importer->GetRootNode ());
    if (profiling) {
        profile.Add (NativePhase::Final, emscripten_get_now () - nativeStart);
        static const char* names[] = { "input-copy", "file-parse", "document-transfer-healing", "bounding-box", "tessellation", "lod-preflight", "export", "first-complete-coarse", "complete-final" };
        emscripten::val spans = emscripten::val::array ();
        int index = 0;
        for (size_t phase = 0; phase < static_cast<size_t>(NativePhase::Count); ++phase) for (int tier = 0; tier < 3; ++tier) {
            const NativeMetric& metric = profile.metrics[phase][tier];
            if (!metric.calls) continue;
            emscripten::val span = emscripten::val::object ();
            span.set ("phase", phase == static_cast<size_t>(NativePhase::FileParse) && brep != nullptr ? "file-parse-topology" : names[phase]); span.set ("tier", phase <= static_cast<size_t>(NativePhase::TransferHealing) ? -1 : tier); span.set ("durationMs", metric.milliseconds); span.set ("calls", metric.calls);
            span.set ("inclusive", phase == static_cast<size_t>(NativePhase::LodPreflight) || phase == static_cast<size_t>(NativePhase::Export) || phase >= static_cast<size_t>(NativePhase::FirstCoarse));
            span.set ("wasmCommittedBytes", metric.committed); span.set ("allocatorLiveSamples", metric.liveSamples);
            if (metric.liveSamples) span.set ("allocatorLiveSampledMaximumBytes", metric.liveSampledMaximum);
            else span.set ("allocatorLiveSampledMaximumBytes", emscripten::val::null ());
            spans.set (index++, span);
        }
        onProfile (spans);
    }
    return result;
}

static ImportParams GetImportParams (const emscripten::val& paramsVal)
{
    ImportParams params;
    if (paramsVal.isUndefined () || paramsVal.isNull ()) {
        return params;
    }

    if (paramsVal.hasOwnProperty ("linearUnit")) {
        emscripten::val linearUnit = paramsVal["linearUnit"];
        std::string linearUnitStr = linearUnit.as<std::string> ();
        if (linearUnitStr == "millimeter") {
            params.linearUnit = ImportParams::LinearUnit::Millimeter;
        } else if (linearUnitStr == "centimeter") {
            params.linearUnit = ImportParams::LinearUnit::Centimeter;
        } else if (linearUnitStr == "meter") {
            params.linearUnit = ImportParams::LinearUnit::Meter;
        } else if (linearUnitStr == "inch") {
            params.linearUnit = ImportParams::LinearUnit::Inch;
        } else if (linearUnitStr == "foot") {
            params.linearUnit = ImportParams::LinearUnit::Foot;
        }
    }

    if (paramsVal.hasOwnProperty ("linearDeflectionType")) {
        emscripten::val linearDeflectionType = paramsVal["linearDeflectionType"];
        std::string linearDeflectionTypeStr = linearDeflectionType.as<std::string> ();
        if (linearDeflectionTypeStr == "bounding_box_ratio") {
            params.linearDeflectionType = ImportParams::LinearDeflectionType::BoundingBoxRatio;
        } else if (linearDeflectionTypeStr == "absolute_value") {
            params.linearDeflectionType = ImportParams::LinearDeflectionType::AbsoluteValue;
        }
    }

    if (paramsVal.hasOwnProperty ("linearDeflection")) {
        emscripten::val linearDeflection = paramsVal["linearDeflection"];
        params.linearDeflection = linearDeflection.as<double> ();
    }

    if (paramsVal.hasOwnProperty ("angularDeflection")) {
        emscripten::val angularDeflection = paramsVal["angularDeflection"];
        params.angularDeflection = angularDeflection.as<double> ();
    }

    return params;
}

emscripten::val ReadStepFile (const emscripten::val& buffer, const emscripten::val& params)
{
    ImporterPtr importer = std::make_shared<ImporterStep> ();
    ImportParams importParams = GetImportParams (params);
    return ImportFile (importer, buffer, importParams, params);
}

emscripten::val ReadIgesFile (const emscripten::val& buffer, const emscripten::val& params)
{
    ImporterPtr importer = std::make_shared<ImporterIges> ();
    ImportParams importParams = GetImportParams (params);
    return ImportFile (importer, buffer, importParams, params);
}

emscripten::val ReadBrepFile (const emscripten::val& buffer, const emscripten::val& params)
{
    ImporterPtr importer = std::make_shared<ImporterBrep> ();
    ImportParams importParams = GetImportParams (params);
    return ImportFile (importer, buffer, importParams, params);
}

emscripten::val ReadFile (const std::string& format, const emscripten::val& buffer, const emscripten::val& params)
{
    if (format == "step") {
        return ReadStepFile (buffer, params);
    } else if (format == "iges") {
        return ReadIgesFile (buffer, params);
    } else if (format == "brep") {
        return ReadBrepFile (buffer, params);
    } else {
        emscripten::val resultObj (emscripten::val::object ());
        resultObj.set ("success", false);
        return resultObj;
    }
}

EMSCRIPTEN_BINDINGS (occtimportjs)
{
    emscripten::function<emscripten::val, const std::string&, const emscripten::val&, const emscripten::val&> ("ReadFile", &ReadFile);

    emscripten::function<emscripten::val, const emscripten::val&, const emscripten::val&> ("ReadStepFile", &ReadStepFile);
    emscripten::function<emscripten::val, const emscripten::val&, const emscripten::val&> ("ReadIgesFile", &ReadIgesFile);
    emscripten::function<emscripten::val, const emscripten::val&, const emscripten::val&> ("ReadBrepFile", &ReadBrepFile);
}

#endif
