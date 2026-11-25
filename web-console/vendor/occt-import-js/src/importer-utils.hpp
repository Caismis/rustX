#pragma once

#include "importer.hpp"

#include <TopoDS_Shape.hxx>
#include <TopoDS_Face.hxx>
#include <TopLoc_Location.hxx>
#include <Poly_Triangulation.hxx>
#include <UnitsMethods_LengthUnit.hxx>

#include <TopTools_DataMapOfShapeReal.hxx>

// Import-scoped; keys retain OCCT shape identity and location across tiers.
class ShapeSizeCache
{
public:
    TopTools_DataMapOfShapeReal sizes;
};

#include <vector>
#include <fstream>

class VectorBuffer : public std::streambuf
{
public:
    VectorBuffer (const std::vector<uint8_t>& v);
};

class OcctFace : public Face
{
public:
    OcctFace (const TopoDS_Face& face);

    virtual size_t VertexCount () const override;
    virtual size_t NormalCount () const override;
    virtual size_t TriangleCount () const override;
    virtual bool HasNormals () const override;
    virtual bool GetColor (Color& color) const override;

    virtual void EnumerateVertices (const std::function<void (double, double, double)>& onVertex) const override;
    virtual void EnumerateNormals (const std::function<void (double, double, double)>& onNormal) const override;
    virtual void EnumerateTriangles (const std::function<void (int, int, int)>& onTriangle) const override;

protected:
    bool HasTriangulation () const;

    const TopoDS_Face& face;
    Handle (Poly_Triangulation) triangulation;
    TopLoc_Location location;
};

UnitsMethods_LengthUnit LinearUnitToLengthUnit (ImportParams::LinearUnit linearUnit);
bool TriangulateShape (TopoDS_Shape& shape, const ImportParams& params);
