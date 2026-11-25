#include "importer-step.hpp"
#include "importer-utils.hpp"
#include "native-profile.hpp"

#include <TDocStd_Document.hxx>
#include <STEPCAFControl_Reader.hxx>

ImporterStep::ImporterStep () :
    ImporterXcaf ()
{

}

bool ImporterStep::TransferToDocument (const std::vector<std::uint8_t>& fileContent)
{
    STEPCAFControl_Reader stepCafReader;
    stepCafReader.SetColorMode (true);
    stepCafReader.SetNameMode (true);

    STEPControl_Reader& stepReader = stepCafReader.ChangeReader ();
    std::string dummyFileName = "stp";
    VectorBuffer inputBuffer (fileContent);
    std::istream inputStream (&inputBuffer);
    IFSelect_ReturnStatus readStatus;
    { NativeSpan span (NativePhase::FileParse);
        readStatus = stepReader.ReadStream (dummyFileName.c_str (), inputStream);
    }
    if (readStatus != IFSelect_RetDone) {
        return false;
    }

    NativeSpan transferSpan (NativePhase::TransferHealing);
    if (!stepCafReader.Transfer (document)) {
        return false;
    }

    return true;
}
