#ifdef EMSCRIPTEN

#include "js-interface.hpp"
#include "importer-step.hpp"
#include "importer-iges.hpp"
#include "importer-brep.hpp"
#include <emscripten/bind.h>

#include <array>

// Bounded streaming copy through public Embind and JavaScript array APIs.
// One fixed stack chunk avoids a second mesh-sized native output allocation.
class NumericArrayWriter
{
public:
    explicit NumericArrayWriter (emscripten::val& array) :
        mArray (array),
        mPush (emscripten::val::global ("Array")["prototype"]["push"]),
        mSize (0)
    {
        // Initialize the generic invoker before creating a memory view.
        mPush.call<void> ("apply", mArray, emscripten::val::array ());
    }
    void Append (double value)
    {
        mValues[mSize++] = value;
        if (mSize == mValues.size ()) {
            Flush ();
        }
    }
    void Flush ()
    {
        if (mSize == 0) {
            return;
        }
        emscripten::val view (emscripten::typed_memory_view (mSize, mValues.data ()));
        mPush.call<void> ("apply", mArray, view);
        mSize = 0;
    }
private:
    emscripten::val& mArray;
    emscripten::val mPush;
    size_t mSize;
    std::array<double, 4096> mValues;
};

class HierarchyWriter
{
public:
    HierarchyWriter (emscripten::val& meshesArr) :
        mMeshesArr (meshesArr),
        mMeshCount (0)
    {
    }

    void WriteNode (const NodePtr& node, emscripten::val& nodeObj)
    {
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

            emscripten::val positionArr (emscripten::val::array ());
            emscripten::val normalArr (emscripten::val::array ());
            emscripten::val indexArr (emscripten::val::array ());
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
                    indices.Append (vertexOffset + v0);
                    indices.Append (vertexOffset + v1);
                    indices.Append (vertexOffset + v2);
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
        bytes += amount;
        if (bytes > 8 * 1024 * 1024) {
            throw LodBudgetExceeded ();
        }
    }

    void Check (const NodePtr& node)
    {
        if (++nodes > 512) {
            throw LodBudgetExceeded ();
        }
        AddBytes (256 + node->GetName ().size () * 4);
        if (node->IsMeshNode ()) {
            node->EnumerateMeshes ([&](const Mesh& mesh) {
                if (++meshes > 256) {
                    throw LodBudgetExceeded ();
                }
                AddBytes (512 + mesh.GetName ().size () * 4);
                mesh.EnumerateFaces ([&](const Face& face) {
                    AddBytes (128);
                    face.EnumerateVertices ([&](double, double, double) {
                        // Positions plus normals, including generated normals.
                        AddBytes (6 * sizeof (double));
                    });
                    face.EnumerateTriangles ([&](int, int, int) {
                        if (++triangles > 65536) {
                            throw LodBudgetExceeded ();
                        }
                        AddBytes (3 * sizeof (double));
                    });
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
    emscripten::val result (emscripten::val::object ());
    emscripten::val root (emscripten::val::object ());
    emscripten::val meshes (emscripten::val::array ());
    HierarchyWriter writer (meshes);
    writer.WriteNode (rootNode, root);
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
    ImportParams params = finalParams;
    if (reportLod) {
        params.linearDeflectionType = ImportParams::LinearDeflectionType::BoundingBoxRatio;
        params.linearDeflection = 0.02;
        params.angularDeflection = 1.0;
    }

    std::vector<uint8_t> bufferArr = emscripten::convertJSArrayToNumberVector<std::uint8_t> (buffer);
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
        for (int level = 0; level < 2; level++) {
            if (level == 1) {
                params.linearDeflection = 0.005;
                params.angularDeflection = 0.75;
            }
            NodePtr rootNode = importer->GetRootNode ();
            try {
                LodBudget budget;
                budget.Check (rootNode);
                // Preflight has tessellated every root. Reusing those native
                // triangulations during export avoids rebuilding the tier.
                params.resetTriangulation = false;
                onLod (level, WriteResult (rootNode));
            } catch (const LodBudgetExceeded&) {
                // The final full result remains available even when a preview
                // cannot fit the independently bounded optional output.
            }
            params.resetTriangulation = true;
        }
        params = finalParams;
        params.resetTriangulation = true;
    }
    return WriteResult (importer->GetRootNode ());
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
