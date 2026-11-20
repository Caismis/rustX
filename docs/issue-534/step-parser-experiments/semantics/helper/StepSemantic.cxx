#include "StepSemantic.hxx"
#include <STEPControl_Reader.hxx>
#include <StepData_StepModel.hxx>
#include <Standard_Type.hxx>
#include <Standard_Failure.hxx>
#include <emscripten/bind.h>
#include <cstdint>
#include <cstring>
#include <cstdio>
#include <vector>
#include <string>
#include <sstream>
#include <stdexcept>
#include <algorithm>
namespace StepSemantic {
static const size_t MaxInput=16*1024*1024, MaxCanonical=32768, MaxRecords=64, MaxValue=256;
static bool scannerBad,scannerFail,scannerEof;
void ScannerFlags(bool bad,bool fail,bool eof){scannerBad=bad;scannerFail=fail;scannerEof=eof;}
static uint64_t hash1, hash2, entity1,entity2;static size_t records,args,entityCalls;static std::string canonical;static bool firstArg,active,canonicalTruncated;
static std::string quote(const char* p){if(!p)return "null";std::string out="\"";size_t n=std::strlen(p);for(size_t i=0;i<std::min(n,MaxValue);++i){unsigned char c=p[i];if(c=='"'||c=='\\'){out+='\\';out+=c;}else if(c<32){char b[7];std::snprintf(b,sizeof b,"\\u%04x",(unsigned)c);out+=b;}else out+=c;}out+='"';return out;}
static void add(const std::string& s){if(canonical.size()+s.size()<=MaxCanonical)canonical+=s;else canonicalTruncated=true;}
static void bytes(uint64_t& h1,uint64_t& h2,const void* value,size_t n){auto p=static_cast<const unsigned char*>(value);for(size_t i=0;i<n;i++){h1=(h1^p[i])*UINT64_C(1099511628211);h2=(h2+p[i]+1)*UINT64_C(14029467366897019727);}}
static void number(uint64_t& h1,uint64_t& h2,uint64_t v){unsigned char p[8];for(int i=0;i<8;i++)p[i]=(unsigned char)(v>>(8*i));bytes(h1,h2,p,8);}
static void text(uint64_t& h1,uint64_t& h2,const char* p){if(!p){number(h1,h2,UINT64_MAX);return;}size_t n=std::strlen(p);number(h1,h2,n);bytes(h1,h2,p,n);}
static std::string digest(uint64_t h1,uint64_t h2){char b[33];std::snprintf(b,sizeof b,"%016llx%016llx",(unsigned long long)h1,(unsigned long long)h2);return b;}
static void reset(){hash1=entity1=UINT64_C(14695981039346656037);hash2=entity2=UINT64_C(7809847782465536322);records=args=entityCalls=0;canonical="[";scannerBad=scannerFail=scannerEof=false;active=true;canonicalTruncated=false;}
void Record(int nr,const char* id,const char* type,int hasArgs){if(!active)return;records++;number(hash1,hash2,0x52);number(hash1,hash2,nr);text(hash1,hash2,id);text(hash1,hash2,type);number(hash1,hash2,hasArgs);firstArg=true;if(records<=MaxRecords)add((records>1?",":"")+std::string("{\"nr\":")+std::to_string(nr)+",\"id\":"+quote(id)+",\"type\":"+quote(type)+",\"hasArgs\":"+std::to_string(hasArgs)+",\"args\":[");else canonicalTruncated=true;}
void Argument(int type,const char* value){if(!active)return;args++;number(hash1,hash2,0x41);number(hash1,hash2,type);text(hash1,hash2,value);if(records<=MaxRecords){add((firstArg?"":",")+std::string("{\"type\":")+std::to_string(type)+",\"length\":"+std::to_string(value?std::strlen(value):0)+",\"value\":"+quote(value)+"}");if(value&&std::strlen(value)>MaxValue)canonicalTruncated=true;}firstArg=false;}
void EndRecord(){if(active){number(hash1,hash2,0x45);if(records<=MaxRecords)add("]}");}}
void Entity(int nr,const char* type){if(!active)return;entityCalls++;number(entity1,entity2,nr);text(entity1,entity2,type);}
class InputBuffer:public std::streambuf {std::vector<char>& data;size_t at,limit;bool fault;public:bool faultTriggered;InputBuffer(std::vector<char>& v,size_t l,bool f):data(v),at(0),limit(l),fault(f),faultTriggered(false){}int_type underflow() override{if(at>=limit){if(fault){faultTriggered=true;throw std::ios_base::failure("injected stream read failure");}return traits_type::eof();}return traits_type::to_int_type(data[at]);}int_type uflow() override{int_type c=underflow();if(!traits_type::eq_int_type(c,traits_type::eof()))++at;return c;}std::streamsize xsgetn(char* out,std::streamsize n)override{std::streamsize i=0;for(;i<n;i++){int_type c=uflow();if(traits_type::eq_int_type(c,traits_type::eof()))break;out[i]=traits_type::to_char_type(c);}return i;}};
static emscripten::val ParseOnly(const emscripten::val& input,int stop,bool fault){using emscripten::val;size_t n=input["length"].as<size_t>();if(n>MaxInput)throw std::runtime_error("bounded diagnostic input limit exceeded");std::vector<char> owned(n);for(size_t i=0;i<n;i++)owned[i]=(char)input[i].as<unsigned>();size_t limit=stop<0?n:std::min(n,(size_t)stop);InputBuffer buffer(owned,limit,fault);std::istream stream(&buffer);reset();STEPControl_Reader reader;int status=-999;std::string exception;try{status=(int)reader.ReadStream("diagnostic.step",stream);}catch(const Standard_Failure& e){exception=std::string("Standard_Failure:")+e.GetMessageString();}catch(const std::exception& e){exception=std::string("std::exception:")+e.what();}catch(...){exception="unknown";}active=false;add("]");val out=val::object();out.set("status",status);out.set("bad",stream.bad());out.set("fail",stream.fail());out.set("eof",stream.eof());out.set("faultTriggered",buffer.faultTriggered);out.set("scannerBad",scannerBad);out.set("scannerFail",scannerFail);out.set("scannerEof",scannerEof);out.set("exception",exception);out.set("recordCount",(double)records);out.set("argumentCount",(double)args);out.set("rawDigest",digest(hash1,hash2));out.set("entityLoadCount",(double)entityCalls);out.set("entityLoadDigest",digest(entity1,entity2));out.set("canonical",canonical);out.set("canonicalTruncated",canonicalTruncated);auto model=reader.StepModel();int count=model.IsNull()?0:model->NbEntities();out.set("entityCount",count);uint64_t m1=UINT64_C(14695981039346656037),m2=UINT64_C(7809847782465536322);val types=val::array();for(int i=1;i<=count;i++){auto entity=model->Value(i);const char* type=entity.IsNull()?"<null>":entity->DynamicType()->Name();number(m1,m2,i);text(m1,m2,type);if(i<=64)types.set(i-1,type);}out.set("modelEntityDigest",digest(m1,m2));out.set("modelEntityTypes",types);return out;}
}
EMSCRIPTEN_BINDINGS(step_semantic_diagnostic){emscripten::function("DiagnosticParseOnly",&StepSemantic::ParseOnly);}
