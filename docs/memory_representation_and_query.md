# In-Memory Data Representation and Query Considerations

This document talks about how data is represented in-memory in this project. It also presents some common data and query patterns that we expect that might have impact on the format. This is a work in progress and for now just represents Paul's brain dump on things he's been looking at.

The ideal situation for Delorean is that it holds most of the data it needs for queries in memory. Most InfluxDB queries hit data from the last 2 hours.

## Data Shape

Let's take four different looking data sets/workloads to look at our different options. Even though these are specific to the infrastructure and application monitoring use case, I think the shape of data represented in these four cases cover all our other use cases like IoT, financial data, and general analytics data.

### Telegraf & Application Metrics Data

* 2000 measurements
* Average of 5 tags per measurement
* Average of 10 fields per measurement
* Tag with min cardinality - 4
* Tag with max cardinality - 1M active and changing
* 10s collection intervals, 500K rows/sec (5M fields/sec)

### Prometheus Data

* 1 measurement with 10k columns (mix of tags and fields)
* Average of 5 tags with values for any given row
* Average of 10 fields with values for any given row
* 10s to 1m collection intervals

### Tracing Data

* Tags with infinite cardinality (trace_id, span_id)
* Lower cardinality tags like host (10k), service (20), region (4)
* Event driven, thousands to millions/sec

### Log Data

* Tags like host, service, region, level
* Unstructured string data
* Event driven, thousands to millions/sec

### Data Shape

Our current assumed model is that we'll be organizing data so that a measurement is a table and tags and fields are columns in that table. Tables will map to individual Parquet files. Further, data will be partitioned, which the operator will have control over. For example, a default partitioning scheme could break up data into 2 hour blocks of time. That means that for each 2 hour block, for each measurement, you'd have a single Parquet file.

For the in-memory representation, I see three possible problem areas or spots for optimization:
1. Sparse Data (the rows in the Prometheus table would consist mostly of nulls, unless we further partition Prometheus data by some other criteria: for example, you could partition a measurement further into a measurement only with fields that have a matching first part of their field name `/(^.+)_.+/`)
1. Repeated values (depending on the sort order, the tag values will be repeated. Since we use dictionary encoding, RLE can help here)
1. Unstructured string data (e.g. logs. Should we be keeping this compressed in blocks in memory?)

Here are the primary options I can think of that we can pursue for how to represent this data for querying from memory:
1. Parquet + partial query results cache (just query from Parquet directly)
1. Arrow RecordBatches
1. Custom format with our own optimizations

Regardless of which option we go with, I think that we're going to end up needing our own custom format, at least for the recent data. Since data is constantly streaming in, we'll need some collection that can be updated as data arrives. Arrow and Parquet are more suited for data that has moved into an immutable state (or mostly, we can handle mutations like deletes and other things with markers that can be resolved at query time and cleaned up with compactions).

So we'll need to be able to query across that hot buffered data, the in-memory immutable data, and the Parquet data. Partitions seem like the abstraction here so we'll need to be able to merge results from multiple partitions.

My first pick would have been Arrow because I'd like to build off any other tooling that community builds over time. It's growing all the time and there seems to be real momentum in the analytics space to use Arrow as a common spec. However, our issues on sparseness and our desire to keep things compact in memory present a problem for Arrow.

There does seem to be a desire in the Arrow community to add to the spec some optimizations for compression. See this thread where a user submitted a PR to talk about these issues:
https://lists.apache.org/thread.html/a99124e57c14c3c9ef9d98f3c80cfe1dd25496bf3ff7046778add937%40%3Cdev.arrow.apache.org%3E

Adding RLE seems to be the biggest concern there. There doesn't seem to be a plan to address the issue of sparseness (many nulls). There is a more recent thread here that brings up the subject, but doesn't get anywhere:
https://lists.apache.org/thread.html/r1d9d707c481c53c13534f7c72d75c7a90dc7b2b9966c6c0772d0e416%40%3Cdev.arrow.apache.org%3E

These threads are long, but they're worth reading. They also link to some papers and blog posts that are interesting and relevant for us.

The truth is we may be limited on what we can do with Arrow. Arrow has a design requirement that it allow constant time access to any element ([spec](https://github.com/apache/arrow/blob/master/docs/source/format/Columnar.rst)). That will certainly limit our compression options.

I'm not sure what would be possible in terms of hanlding nulls for primitive types. In the project I see that adding nulls to primitive types touches the bitset that maps nulls, but also allocates space for the base type. I don't believe this is true for the variable length byte array types so maybe we can do something similar here and suggest the change?

Either way, if these are modifications to the spec, they'll take time to get through. We can always have a fork until things get merged upstream. The C++ and Java implementations of Arrow are their canonical implementations so adding support to one or both of those would help things move along faster.

The other option is that we use Extensions and layer our logic outside of Arrow in our application layer ([see extensions](https://arrow.apache.org/docs/format/Columnar.html#format-metadata-extension-types)). Of course that then reduces the utility of what we'd be able to use from the Arrow ecosystem. We'd also likely change from those custom extended column types to built-in Arrow types when transfering across the wire (using Flight) to non-delorean clients.

Here are some questions we could answer that I think might help make the decision of what to do:
1. What is the memory usage for keeping data of each scenario in memory in Arrow vs. a compressed format?
1. Can partitioning solve some of the problems with Arrow representation and memory?
1. How useful are the existing Arrow query tools for solving our needs with Delorean?
1. What is the performance impact of querying over compressed vs. raw data?

### Target Hardware and Data Shape

Another bonus consideration here is our target hardware. We plan on running on systems that have abundant memory, locally attached NVMe SSD drives, and object storage. With the locally attached NVMe SSD drives and their significant peformance, we may do well to consider an in-memory format that can also be written out to disk and MMAP'd. We could use Parquet, but I'm not sure that's the best option here since we likely won't be bandwidth constrained. Ideally we could use the same query code/path to work with the in-memory & MMAP'd files.

## Partitioning

For partitioning data, there are potentially multiple ways we may want to partition it:
1. Partitioning over the network
1. Partitioning in memory
1. Paritioining into Parquet files on S3

I'd like to come up with a way of specifying partitioning that can be used for any of those three scenarios. I can imagine that we may want it to be different for each one depedning on how things look, but I'm not sure. I think that what queries we're optimizing will be the thing that has the biggest impact on how we do partitioning.

You should be able to specify a partitioning scheme like these:
* partition into two hour blocks, then by table
* partition into 24 hour blocks, then by table, then by region column
* partition into tables
* partition into 2 hour blocks, then table, then by non-dictionary columns that match /(^.+)_.+/
* partition into 2 hour blocks, then table, then lowest cardinality dictionary column

Note that my partitioning examples all use the table organization, which Influx' schema maps onto.

I think partitioning is important because of the way I'm thinking about querying. Since we have a requirement for infinite cardinality data and tiered data, we need to break things up. I think of querying as a brute force thing that happens over all the data in the partitions that get selected for a query.

## Querying

Here are some common queries we're likely to see
1. Cpu data for a specific host for the last hour grouped by cpu
1. rate of HTTP request counters for each host running a specific service for the last hour
1. Aggregate across some low cardinality column for the last 24 hours

The shape of metrics data is important to keep in mind when thinking about querying. See the different [Prometheus metrics types](https://prometheus.io/docs/concepts/metric_types/).

With counters, they only ever make sense if you first group by all tag values, then do some sort of calculation on them (first, last, derivative). That's because they're monotonically incresing things so you have to take care to handle them as an individual series before you combine them together (like when you sum them to get a total request rate across all hosts).

How we partition data may ultimately come down to how many records/sec we can expect the query engine to handle for these different scenarios. Not all queries would be able to rule out many partitions. For example, say we were partitioning based on time, then table, then the region column. A query like this

```
select min(foo) from bar where time > -3h and region = 1
```

Would be able to narrow it down to the minimum set of partitions. However, a query like this:

```
select min(foo) from bar where time > -3h and host == "a"
```

Wouldn't be able to narrow down past the time dimension. These kinds of queries would be common so simply partitioning based on cardinality of some tags may not get us very far. However, we could potentially cache what partitions actually had data that matched for a given query and then use that for future queries to narrow down the set.

## Other Concerns

I have my doubts that the gRPC interface to Flux will enable us to show the kinds of performance gains that we're hoping to achieve. Should we be updating Flux to use Arrow Flight? I'm not sure that will give us the result we want either as I'm not sure the Flux engine isn't the n breaking up that data into many different series.

This really depends on if we are starting out just miroring the storage server interface, or if we're adopting an existing query engine and folding it into Delorean. If it's the latter, then I think we'd do best to pick some Flux queries and data and show their performace with queryd + storage, then represent the same queries directly to Delorean and show that performance. We may be able to do this with queryd and Flux by just hacking in a few pushdown rules.
