#include "step-transfer.hxx"

#include <Interface_EntityIterator.hxx>
#include <Interface_Graph.hxx>
#include <STEPControl_Reader.hxx>
#include <StepBasic_ProductDefinition.hxx>
#include <StepDimTol_DatumFeature.hxx>
#include <StepDimTol_GeometricTolerance.hxx>
#include <StepRepr_NextAssemblyUsageOccurrence.hxx>
#include <StepRepr_ProductDefinitionShape.hxx>
#include <StepRepr_PropertyDefinition.hxx>
#include <StepRepr_Representation.hxx>
#include <StepRepr_RepresentationItem.hxx>
#include <StepRepr_ShapeAspect.hxx>
#include <StepShape_AdvancedBrepShapeRepresentation.hxx>
#include <StepShape_ConnectedFaceSet.hxx>
#include <StepShape_Face.hxx>
#include <StepShape_FaceBasedSurfaceModel.hxx>
#include <StepShape_FacetedBrepShapeRepresentation.hxx>
#include <StepShape_GeometricallyBoundedSurfaceShapeRepresentation.hxx>
#include <StepShape_ManifoldSolidBrep.hxx>
#include <StepShape_ManifoldSurfaceShapeRepresentation.hxx>
#include <StepShape_ShapeDefinitionRepresentation.hxx>
#include <StepShape_ShellBasedSurfaceModel.hxx>
#include <TopoDS_Shape.hxx>
#include <TopExp_Explorer.hxx>
#include <TransferBRep.hxx>
#include <Transfer_TransientProcess.hxx>
#include <XSControl_TransferReader.hxx>
#include <XSControl_WorkSession.hxx>

#include <stdexcept>
#include <unordered_map>
#include <unordered_set>
#include <vector>

namespace {
struct Declaration {
    bool geometry = false;
    bool surface = false;
    bool missingRepresentation = false;
    std::vector<Handle(StepBasic_ProductDefinition)> occurrences;
};

bool declaresFaces(const Handle(Standard_Transient)& item) {
    return !item.IsNull() && (
        item->IsKind(STANDARD_TYPE(StepShape_AdvancedBrepShapeRepresentation))
        || item->IsKind(STANDARD_TYPE(StepShape_FacetedBrepShapeRepresentation))
        || item->IsKind(STANDARD_TYPE(StepShape_ManifoldSurfaceShapeRepresentation))
        || item->IsKind(STANDARD_TYPE(StepShape_GeometricallyBoundedSurfaceShapeRepresentation))
        || item->IsKind(STANDARD_TYPE(StepShape_ManifoldSolidBrep))
        || item->IsKind(STANDARD_TYPE(StepShape_ShellBasedSurfaceModel))
        || item->IsKind(STANDARD_TYPE(StepShape_FaceBasedSurfaceModel))
        || item->IsKind(STANDARD_TYPE(StepShape_ConnectedFaceSet))
        || item->IsKind(STANDARD_TYPE(StepShape_Face)));
}

bool representationDeclaresFaces(const Handle(StepRepr_Representation)& representation) {
    if (declaresFaces(representation)) return true;
    for (int i = 1; i <= representation->NbItems(); ++i)
        if (declaresFaces(representation->ItemsValue(i))) return true;
    return false;
}

Handle(StepBasic_ProductDefinition) productOf(const Handle(StepShape_ShapeDefinitionRepresentation)& definition,
                                              const Interface_Graph& graph) {
    const auto property = definition->Definition().PropertyDefinition();
    if (property.IsNull()) return {};
    const auto characterized = property->Definition();
    if (auto product = characterized.ProductDefinition(); !product.IsNull()) return product;
    const auto aspect = characterized.ShapeAspect();
    if (aspect.IsNull() || aspect->IsKind(STANDARD_TYPE(StepDimTol_DatumFeature)) || aspect->OfShape().IsNull()) return {};
    for (auto uses = graph.Sharings(aspect); uses.More(); uses.Next())
        if (uses.Value()->IsKind(STANDARD_TYPE(StepDimTol_GeometricTolerance))) return {};
    return aspect->OfShape()->Definition().ProductDefinition();
}

void requireShape(const TopoDS_Shape& shape, bool surface) {
    if (shape.IsNull() || (surface && !TopExp_Explorer(shape, TopAbs_FACE).More()))
        throw std::runtime_error("Incomplete STEP transfer");
}
}

void verifyStepTransfer(STEPControl_Reader& reader) {
    const auto& transfer = reader.WS()->TransferReader();
    const auto& process = transfer->TransientProcess();
    const auto& model = reader.Model();
    std::unordered_map<const StepBasic_ProductDefinition*, Declaration> products;
    for (int i = 1; i <= model->NbEntities(); ++i) {
        const auto& entity = model->Value(i);
        if (auto definition = Handle(StepShape_ShapeDefinitionRepresentation)::DownCast(entity); !definition.IsNull()) {
            const auto product = productOf(definition, process->Graph());
            if (product.IsNull()) continue;
            auto& declaration = products[product.get()];
            declaration.geometry = true;
            const auto representation = definition->UsedRepresentation();
            if (representation.IsNull()) declaration.missingRepresentation = true;
            else declaration.surface |= representationDeclaresFaces(representation);
        } else if (auto occurrence = Handle(StepRepr_NextAssemblyUsageOccurrence)::DownCast(entity); !occurrence.IsNull()) {
            const auto parent = occurrence->RelatingProductDefinition();
            if (!parent.IsNull()) products[parent.get()].occurrences.push_back(occurrence->RelatedProductDefinition());
        }
    }

    std::vector<Handle(StepBasic_ProductDefinition)> pending;
    for (int i = 1; i <= reader.NbRootsForTransfer(); ++i) {
        const auto root = reader.RootForTransfer(i);
        if (auto product = Handle(StepBasic_ProductDefinition)::DownCast(root); !product.IsNull()) {
            pending.push_back(product);
        } else {
            auto representation = Handle(StepRepr_Representation)::DownCast(root);
            if (auto definition = Handle(StepShape_ShapeDefinitionRepresentation)::DownCast(root); !definition.IsNull()) {
                representation = definition->UsedRepresentation();
                if (representation.IsNull()) throw std::runtime_error("Missing STEP shape representation");
            }
            const bool surface = representation.IsNull() ? declaresFaces(root) : representationDeclaresFaces(representation);
            requireShape(transfer->ShapeResult(root), surface);
        }
    }
    // A metadata-only product has no shape declaration or assembly children.
    // Explicit wire/point representations still require a transferred shape,
    // and only declarations of surfaces require that shape to contain faces.
    std::unordered_set<const StepBasic_ProductDefinition*> visited;
    while (!pending.empty()) {
        const auto product = pending.back();
        pending.pop_back();
        if (product.IsNull()) throw std::runtime_error("Missing STEP assembly occurrence");
        if (!visited.insert(product.get()).second) continue;
        const auto entry = products.find(product.get());
        if (entry == products.end()) continue;
        const auto& declaration = entry->second;
        if (declaration.missingRepresentation) throw std::runtime_error("Missing STEP shape representation");
        if (declaration.geometry || !declaration.occurrences.empty())
            requireShape(TransferBRep::ShapeResult(process->Find(product)), declaration.surface);
        pending.insert(pending.end(), declaration.occurrences.begin(), declaration.occurrences.end());
    }
}
