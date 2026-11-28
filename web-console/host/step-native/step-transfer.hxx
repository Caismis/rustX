#pragma once

class STEPControl_Reader;

// Check the entity-level coverage left by STEPCAFControl_Reader::Transfer.
void verifyStepTransfer(STEPControl_Reader& reader);
